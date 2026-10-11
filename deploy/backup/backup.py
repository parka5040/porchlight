#!/usr/bin/env python3
"""Independent backup producer. Errors are sanitized; diagnostics never echo DB/key values."""
from contextlib import contextmanager
import datetime as dt
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import ssl
import stat
import subprocess as sp
import sys
import tarfile
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / 'lib'))
import operations as ops
import blob_integrity


class BackupInterrupted(BaseException):
    pass


children = set()


def spawn(*args, **kwargs):
    p = sp.Popen(*args, start_new_session=True, **kwargs)
    children.add(p)
    return p


def stop_child(p):
    if p.poll() is None:
        os.killpg(p.pid, signal.SIGTERM)
        try:
            p.wait(timeout=2)
        except sp.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.wait()
    children.discard(p)


def command(args, input=None, text=False, env=None, capture=False):
    p = spawn(args, stdin=sp.PIPE if input is not None else sp.DEVNULL,
              stdout=sp.PIPE if capture else sp.DEVNULL, stderr=sp.DEVNULL, text=text, env=env)
    try:
        with bounded_phase():
            output, _ = p.communicate(input, timeout=600)
        if p.returncode: raise sp.CalledProcessError(p.returncode, args)
        return output
    finally:
        stop_child(p)


def interrupted(signum, _):
    signal.signal(signum, signal.SIG_IGN)
    # Never wait in a signal handler: Popen.wait may already hold its non-reentrant wait lock.
    for p in list(children):
        try: os.killpg(p.pid, signal.SIGTERM)
        except ProcessLookupError: pass
    raise BackupInterrupted()


def work_root(backups, disk=False):
    # Dump is always on disk: its size is not bounded by the small-secret tmpfs cap.
    variable = 'PORCHLIGHT_DUMP_TMPDIR' if disk else 'TMPDIR'
    default = '/dumpwork' if disk else tempfile.gettempdir()
    root = Path(os.environ.get(variable, default)) / 'porchlight-backup'
    if root.resolve() == backups.resolve() or backups.resolve() in root.resolve().parents:
        raise ValueError('backup work directory must be outside published backups')
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    if any(p.is_symlink() for p in (root, *root.parents)) or root.stat().st_uid != os.getuid() or root.stat().st_mode & 0o077:
        raise ValueError('backup work directory must be private')
    return root


def recover_locked(backups):
    # Called only under the shared filesystem lock: no active run can be mistaken for stale.
    root = work_root(backups)
    for directory in (root, work_root(backups, disk=True)):
        for p in directory.glob('work-*'):
            if p.is_symlink() or p.is_file(): p.unlink()
            elif p.is_dir(): shutil.rmtree(p)
    # Upgrade cleanup for old releases that staged plaintext here.
    for p in backups.glob('.work-*'):
        if p.is_symlink() or p.is_file(): p.unlink()
        elif p.is_dir(): shutil.rmtree(p)
    for p in backups.glob('.partial-*'): p.unlink()
    # Free disposable disk space even if PostgreSQL cannot write (e.g. after ENOSPC).
    # A failed repair still aborts this attempt: never start a backup with stale metadata.
    sql("UPDATE porchlight.backup_runs SET status='failed',finished_at=clock_timestamp(),error='backup interrupted; recovered at startup' WHERE status='running';")
    return root


def db_env():
    env = os.environ.copy()
    env.update(PGHOST=os.environ.get('PGHOST', 'postgres'), PGDATABASE='porchlight',
               PGUSER='porchlight_backup', PGPASSWORD=os.environ['PORCHLIGHT_BACKUP_PASSWORD'])
    return env


def sql(query):
    return command(['psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1'],
                   input=query, text=True, env=db_env(), capture=True).strip()


def literal(value):
    return "'" + str(value).replace("'", "''") + "'"


class Snapshot:
    def __enter__(self):
        self.p = spawn(['psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1'], stdin=sp.PIPE,
                          stdout=sp.PIPE, stderr=sp.DEVNULL, text=True, env=db_env())
        try:
            self.query('BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; SELECT pg_export_snapshot();')
            self.id = self.result
            return self
        except BaseException:
            stop_child(self.p)
            self.p.stdin.close()
            self.p.stdout.close()
            raise

    def query(self, query):
        with bounded_phase():
            self.p.stdin.write(query + '\n')
            self.p.stdin.flush()
            self.result = self.p.stdout.readline().strip()
        if not self.result:
            raise RuntimeError('snapshot query failed')
        heartbeat()
        return self.result

    def __exit__(self, *_):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=2)
        finally:
            stop_child(self.p)
            self.p.stdout.close()


active_lease = None


class BlobLease:
    """Each sql() is an independent committed transaction, before snapshot/dump."""
    def __init__(self, holder): self.holder = holder; self.id = None; self.last = 0
    def __enter__(self):
        global active_lease
        self.id = sql("SELECT porchlight.pl_take_blob_lease(" + literal(self.holder) + ");")
        if not re.fullmatch('[0-9a-f-]{36}', self.id): raise ValueError('lease acquisition failed')
        self.last = time.monotonic()
        active_lease = self
        return self
    def renew(self, force=False):
        if force or time.monotonic() - self.last >= 300:
            if sql("SELECT porchlight.pl_renew_blob_lease(" + literal(self.id) + ");") != 't':
                raise ValueError('blob lease expired; refusing publication')
            self.last = time.monotonic()
    def __exit__(self, *_):
        global active_lease
        active_lease = None
        # On a disconnected/crashed database the bounded lease expires naturally.
        try: sql("DELETE FROM porchlight.blob_leases WHERE id=" + literal(self.id) + ";")
        except Exception: print('backup blob lease release failed', file=sys.stderr)


def referenced_blobs(snapshot, lease_id):
    # Final S7b tables only. A post-lease newly referenced blob fails closed;
    # a subsequent backup retries with a fresh lease rather than copying unpinned bytes.
    rows = json.loads(snapshot.query("SELECT COALESCE(json_agg(to_jsonb(b)), '[]') FROM porchlight.blobs b JOIN porchlight.blob_lease_items li ON li.household_id=b.household_id AND li.sha256=b.sha256 WHERE li.lease_id=" + literal(lease_id) + " AND EXISTS(SELECT 1 FROM porchlight.attachments a WHERE a.household_id=b.household_id AND a.deleted_at IS NULL AND (a.blob_sha256=b.sha256 OR a.thumbnail_sha256=b.sha256));"))
    refs = json.loads(snapshot.query("SELECT COALESCE(json_agg(to_jsonb(a)), '[]') FROM porchlight.attachments a WHERE a.deleted_at IS NULL;"))
    # Keep snapshot size/content identity through hashing AND tar output.
    return blob_integrity.metadata(rows, refs)


def copy_private(source, destination):
    with bounded_phase():
        if source.is_symlink() or any(parent.is_symlink() for parent in source.parents):
            raise ValueError('symlink secret or blob path')
        if source.is_dir():
            for p in source.rglob('*'):
                if p.is_symlink():
                    raise ValueError('symlink data')
            shutil.copytree(source, destination)
        else:
            shutil.copy2(source, destination)


class ProgressReader:
    def __init__(self, file):
        self.file = file
        self.digest = hashlib.sha256()
        self.size = 0

    def read(self, size=-1):
        value = self.file.read(size)
        if value:
            self.digest.update(value)
            self.size += len(value)
            heartbeat()
        return value

    def identity(self):
        return {'sha256': self.digest.hexdigest(), 'size': self.size}


def open_private_file(path):
    if any(p.is_symlink() for p in (path, *path.parents)):
        raise ValueError('symlink dump or blob path')
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    if not stat.S_ISREG(os.fstat(fd).st_mode):
        os.close(fd)
        raise ValueError('non-regular dump or blob')
    return os.fdopen(fd, 'rb')


def external_digest(path):
    with open_private_file(path) as f:
        reader = ProgressReader(f)
        while reader.read(1024 * 1024):
            pass
        return reader.identity()


def dump_snapshot(snapshot, destination):
    p = spawn(['pg_dump', '-Fc', '--snapshot=' + snapshot.id, '-f', str(destination)],
              env=db_env(), stdout=sp.DEVNULL, stderr=sp.DEVNULL)
    last_size = 0
    try:
        while p.poll() is None:
            size = destination.stat().st_size if destination.exists() else 0
            if size > last_size:
                heartbeat()  # No unconditional pulse: a wedged dump must become unhealthy.
                last_size = size
            time.sleep(1)
        if p.returncode: raise RuntimeError('dump failed')
        heartbeat()
    finally:
        stop_child(p)


def publication_digest(path, lease):
    # Hashing the encrypted file can be slow too. Refuse publication if the lease
    # expired while hashing; do not revive it or publish an unpinned snapshot.
    digest = ops.digest(path)
    lease.renew(force=True)
    return digest


def run():
    data = Path(os.environ.get('PORCHLIGHT_DATA_DIR', '/data'))
    backups = Path(os.environ.get('PORCHLIGHT_BACKUP_DIR', '/backups'))
    backups.mkdir(mode=0o700, parents=True, exist_ok=True)
    with open(backups / '.backup.lock', 'a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        root = recover_locked(backups)
        run_id = None
        partial = None
        succeeded = False
        try:
            run_id = sql("INSERT INTO porchlight.backup_runs(status) VALUES ('running') RETURNING id;")
            with BlobLease('backup:' + run_id) as lease, tempfile.TemporaryDirectory(prefix='work-', dir=root) as work, tempfile.TemporaryDirectory(prefix='work-', dir=work_root(backups, disk=True)) as disk:
                work = Path(work)
                stage = work / 'bundle'
                stage.mkdir(mode=0o700)
                with Snapshot() as snapshot:
                    metadata = json.loads(snapshot.query("SELECT json_build_object('migration_version',(SELECT max(version) FROM public._sqlx_migrations WHERE success),'household_zone',(SELECT tzid FROM porchlight.households LIMIT 1),'ca_fingerprint',(SELECT encode(ca_fingerprint,'hex') FROM porchlight.install_state WHERE id));"))
                    tables = json.loads(snapshot.query("SELECT json_agg(tablename ORDER BY tablename) FROM pg_tables WHERE schemaname='porchlight';"))
                    counts = {}
                    for table in tables:
                        ident = '"' + table.replace('"', '""') + '"'
                        counts[table] = int(snapshot.query('SELECT count(*) FROM porchlight.' + ident + ';'))
                    blobs = referenced_blobs(snapshot, lease.id)
                    dump = Path(disk) / 'db.dump'
                    dump_snapshot(snapshot, dump)
                    for name in ('pki', 'secrets', 'backup-recipient.txt'):
                        copy_private(data / name, stage / name)
                    # Renewal/rollover may race filesystem copies. Reject inconsistent PKI instead of publishing it.
                    fingerprint = hashlib.sha256(ssl.PEM_cert_to_DER_cert((stage / 'pki/ca.crt').read_text())).hexdigest()
                    if fingerprint != metadata['ca_fingerprint']:
                        raise ValueError('CA differs from database snapshot')
                    for cert, key in [('ca.crt', 'ca.key'), ('server.crt', 'server.key')]:
                        cert_pub = command(['openssl','x509','-in',str(stage/'pki'/cert),'-noout','-pubkey'],capture=True)
                        key_pub = command(['openssl','pkey','-in',str(stage/'pki'/key),'-pubout'],capture=True)
                        if cert_pub != key_pub:
                            raise ValueError('certificate/key changed during backup')
                    copy_private(Path(os.environ.get('PORCHLIGHT_ENV_FILE', '/stack/.env')), stage / '.env')
                    (stage / 'blobs').mkdir()  # Empty archive directory, never attachment staging.
                    external = {'db.dump': dump}
                    external.update({'blobs/' + name: data / 'blobs' / name for name in blobs})
                    metadata.update(table_row_counts=counts, referenced_blobs=list(blobs))
                    manifest = ops.manifest(stage, os.environ['PORCHLIGHT_VERSION'], metadata)
                    for name, path in external.items():
                        digest = external_digest(path)
                        if name.startswith('blobs/'):
                            blob_integrity.check_digest(digest, blobs[name[6:]])
                        manifest['files'][name] = digest
                    (stage / 'manifest.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
                stamp = dt.datetime.now(dt.timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
                filename = 'porchlight-' + stamp + '.tar.age'
                partial = backups / ('.partial-' + filename)
                # Stream plaintext tar directly to age; only encrypted bytes touch backups/.
                with open(partial, 'xb') as output:
                    age = spawn(['age', '-R', str(data / 'backup-recipient.txt')],
                                stdin=sp.PIPE, stdout=output, stderr=sp.DEVNULL)
                    try:
                        with tarfile.open(fileobj=age.stdin, mode='w|') as t:
                            for p in sorted(stage.iterdir()): t.add(p, arcname=p.name)
                            for name, path in sorted(external.items()):
                                with open_private_file(path) as f:
                                    info = t.gettarinfo(fileobj=f, arcname=name)
                                    if not info.isfile(): raise ValueError('non-regular bundle file')
                                    reader = ProgressReader(f)
                                    t.addfile(info, reader)
                                    # Validate the exact streamed bytes too: prehash alone has a race.
                                    blob_integrity.check_digest(reader.identity(), manifest['files'][name])
                                    if f.read(1): raise ValueError('bundle file grew during backup')
                        age.stdin.close()
                        if age.wait(): raise RuntimeError('encryption failed')
                    finally:
                        stop_child(age)
                        if not age.stdin.closed: age.stdin.close()
                lease.renew(force=True)
                encrypted = partial
                with open(encrypted, 'rb') as f:
                    os.fsync(f.fileno())
                digest = publication_digest(encrypted, lease)
                os.rename(encrypted, backups / filename)
                fd = os.open(backups, os.O_DIRECTORY)
                try:
                    os.fsync(fd)
                finally:
                    os.close(fd)
                due = ops.next_due(metadata['household_zone'], ops.now())
                sql("UPDATE porchlight.backup_runs SET status='success', finished_at=clock_timestamp(), bundle_file=" + literal(filename) + ', size=' + str(digest['size']) + ', sha256=' + literal(digest['sha256']) + ', next_due_at=' + literal(due) + ' WHERE id=' + literal(run_id) + ';')
                succeeded = True
            keep = ops.retained([p.name for p in backups.iterdir()])
            for p in backups.glob('porchlight-*.tar.age'):
                if p.name in ops.retained([p.name]) and p.name not in keep:
                    p.unlink()
            print('PASS backup ' + filename, flush=True)
        except BaseException:
            # INSERT may have committed before cancellation interrupted the response; the lock makes this safe.
            if not succeeded:
                where = 'id=' + literal(run_id) if run_id else "status='running'"
                sql("UPDATE porchlight.backup_runs SET status='failed',finished_at=clock_timestamp(),error='backup failed; inspect private runtime diagnostics' WHERE " + where + " AND status='running';")
            raise
        finally:
            if partial is not None: partial.unlink(missing_ok=True)


# Queries and small private copies do not expose byte progress. Their health is
# a fixed phase deadline, not a periodic liveness pulse. Nested work cannot extend it.
phase_active = False


@contextmanager
def bounded_phase():
    global phase_active
    if phase_active:
        yield
        return
    root = Path(os.environ.get('PORCHLIGHT_BACKUP_DIR', '/backups'))
    phase = root / '.phase'
    pending = root / '.phase-pending'
    deadline = time.monotonic() + 600
    pending.write_text(str(deadline))
    pending.chmod(0o600)
    os.replace(pending, phase)
    phase_active = True
    try:
        yield
    finally:
        phase_active = False
        phase.unlink(missing_ok=True)
        # The phase has ended; this is actual main-loop progress, not a timer thread.
        (root / '.heartbeat').touch(mode=0o600)


def heartbeat():
    if active_lease is not None: active_lease.renew()
    Path(os.environ.get('PORCHLIGHT_BACKUP_DIR', '/backups'), '.heartbeat').touch(mode=0o600)


def health():
    global phase_active
    # The probe must not publish/refresh the worker's phase or heartbeat.
    previous = phase_active
    phase_active = True
    try:
        assert sql('SELECT 1;') == '1'
    finally:
        phase_active = previous
    root = Path(os.environ.get('PORCHLIGHT_BACKUP_DIR', '/backups'))
    try:
        deadline = float((root / '.phase').read_text())
    except FileNotFoundError:
        assert time.time() - (root / '.heartbeat').stat().st_mtime < 180
    else:
        assert 0 < deadline - time.monotonic() <= 600


def daemon():
    backups = Path(os.environ.get('PORCHLIGHT_BACKUP_DIR', '/backups'))
    backups.mkdir(mode=0o700, parents=True, exist_ok=True)
    heartbeat()
    active = False
    recovered = False
    try:
        while True:
            heartbeat()
            delay = 60
            try:
                if not recovered:
                    with open(backups / '.backup.lock', 'a') as lock:
                        fcntl.flock(lock, fcntl.LOCK_EX)
                        recover_locked(backups)
                    recovered = True
                latest = json.loads(sql("SELECT COALESCE((SELECT json_build_object('finished_at',finished_at,'next_due_at',next_due_at) FROM porchlight.backup_runs WHERE status='success' ORDER BY finished_at DESC LIMIT 1),'{}'::json);"))
                if ops.backup_due(latest.get('finished_at'), latest.get('next_due_at'), ops.now()):
                    active = True
                    run()
                    active = False
            except Exception:
                active = False
                print('FAIL backup job (details suppressed)', file=sys.stderr, flush=True)
                delay = 3600 # at most one failed attempt per hour, including catch-up
            # Idle/backoff progress is main-loop-only; blocking SQL/dump/tar cannot fake liveness.
            for _ in range(delay // 60):
                heartbeat()
                time.sleep(60)
    except BackupInterrupted:
        if active: raise
        # An ordinary idle docker stop is not a failed backup.
        return


if __name__ == '__main__':
    os.umask(0o077)
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try:
        mode = sys.argv[1] if len(sys.argv) > 1 else 'daemon'
        if mode == 'now':
            run()
        elif mode == 'daemon':
            daemon()
        elif mode == 'health':
            health()
        else:
            raise ValueError('unknown backup mode')
    except BaseException:
        print('FAIL backup job (details suppressed)', file=sys.stderr)
        sys.exit(1)
