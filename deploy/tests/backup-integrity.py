#!/usr/bin/env python3
"""Corrupted content-addressed blobs must never become successful backups."""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import ssl
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('backup', ROOT / 'deploy/backup/backup.py')
backup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(backup)

class Lease:
    id = '00000000-0000-0000-0000-000000000001'
    def __init__(self, *_): pass
    def __enter__(self): return self
    def __exit__(self, *_): pass
    def renew(self, **_): pass

class Age:
    def __init__(self): self.stdin = io.BytesIO()
    def wait(self): return 0

class Tests(unittest.TestCase):
    def test_same_length_corruption_refuses_publication_and_preserves_old_bundle(self):
        self.producer_failure('corrupt')

    def test_size_mismatch_refuses_publication(self):
        self.producer_failure('size')

    def test_blob_mutation_between_prehash_and_tar_read_refuses_publication(self):
        self.producer_failure('race')

    def producer_failure(self, mode):
        (ROOT / 'var/tests').mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=ROOT / 'var/tests') as tmp:
            root = Path(tmp)
            data = root / 'data'; data.mkdir()
            (data / 'pki').mkdir(); (data / 'secrets').mkdir()
            pem = '-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n'
            for name in ['ca.crt', 'server.crt']: (data / 'pki' / name).write_text(pem)
            for name in ['ca.key', 'server.key']: (data / 'pki' / name).write_text('fixture, not a key')
            (data / 'backup-recipient.txt').write_text('fixture-public-recipient')
            envfile = root / '.env'; envfile.write_text('fixture, no credentials')
            digest = hashlib.sha256(b'good').hexdigest()
            blob = data / 'blobs' / digest[:2] / digest[2:4] / digest
            blob.parent.mkdir(parents=True); blob.write_bytes(b'evil' if mode == 'corrupt' else b'good')
            original_digest = backup.external_digest
            def digest_and_mutate(path):
                value = original_digest(path)
                if mode == 'race' and path == blob: blob.write_bytes(b'evil')
                return value
            backups = root / 'backups'; backups.mkdir()
            old = backups / 'porchlight-20200101T000000000000Z.tar.age'; old.write_bytes(b'old-good-bundle')
            class Snapshot:
                def __enter__(self): return self
                def __exit__(self, *_): pass
                def query(self, query):
                    if 'migration_version' in query:
                        return json.dumps(dict(migration_version=63, household_zone='UTC', ca_fingerprint=hashlib.sha256(ssl.PEM_cert_to_DER_cert(pem)).hexdigest()))
                    if 'pg_tables' in query: return '[]'
                    if 'to_jsonb(b)' in query: return json.dumps([dict(household_id='h', sha256=digest, size=5 if mode == 'size' else 4)])
                    if 'to_jsonb(a)' in query: return json.dumps([dict(household_id='h', blob_sha256=digest, thumbnail_sha256=None)])
                    raise AssertionError('unexpected snapshot query')
            env = dict(PORCHLIGHT_DATA_DIR=str(data), PORCHLIGHT_BACKUP_DIR=str(backups),
                       PORCHLIGHT_ENV_FILE=str(envfile), PORCHLIGHT_VERSION='0.1.0',
                       TMPDIR=str(root / 'tmpfs'), PORCHLIGHT_DUMP_TMPDIR=str(root / 'disk'))
            with patch.dict(os.environ, env), patch.object(backup, 'Snapshot', Snapshot), patch.object(backup, 'BlobLease', Lease), \
                 patch.object(backup, 'sql', return_value=Lease.id) as sql, patch.object(backup, 'command', return_value=b'public'), \
                 patch.object(backup, 'dump_snapshot', side_effect=lambda _, path: path.write_bytes(b'dump')), \
                 patch.object(backup, 'spawn', return_value=Age()), patch.object(backup, 'stop_child'), patch.object(backup, 'heartbeat'), \
                 patch.object(backup, 'external_digest', side_effect=digest_and_mutate):
                with self.assertRaises(ValueError): backup.run()
            self.assertEqual(old.read_bytes(), b'old-good-bundle')
            self.assertEqual(list(backups.glob('porchlight-*.tar.age')), [old])
            self.assertFalse(list(backups.glob('.partial-*')))
            self.assertTrue(any("status='failed'" in call.args[0] for call in sql.call_args_list))
            self.assertFalse(any("SET status='success'" in call.args[0] for call in sql.call_args_list))

    def test_restore_drill_rejects_self_consistent_manifest_with_corrupt_blob(self):
        # Execute the trusted drill's actual comparison block, replacing only Docker's SQL transport.
        import re
        (ROOT / 'var/tests').mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=ROOT / 'var/tests') as tmp:
            root = Path(tmp)
            pem = '-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n'
            fingerprint = hashlib.sha256(ssl.PEM_cert_to_DER_cert(pem)).hexdigest()
            digest = hashlib.sha256(b'good').hexdigest()
            data = root / 'stack/data'
            for p in [data / 'pki', root / 'bundle/pki']: p.mkdir(parents=True); (p / 'ca.crt').write_text(pem)
            blob = data / 'blobs' / digest[:2] / digest[2:4] / digest
            blob.parent.mkdir(parents=True); blob.write_bytes(b'evil')
            manifest = dict(table_row_counts={}, migration_version=63, files={'blobs/'+str(blob.relative_to(data / 'blobs')): dict(sha256=hashlib.sha256(b'evil').hexdigest(), size=4)})
            (root / 'manifest.json').write_text(json.dumps(manifest))
            rows = [dict(household_id='h', sha256=digest, size=4)]
            refs = [dict(household_id='h', blob_sha256=digest, thumbnail_sha256=None)]
            def query(*_, input, **__):
                if 'max(version)' in input: return '63'
                if 'NOT success' in input: return '0'
                if 'ca_fingerprint' in input: return fingerprint
                if 'to_jsonb(b)' in input: return json.dumps(rows)
                if 'to_jsonb(a)' in input: return json.dumps(refs)
                raise AssertionError('unexpected restored SQL query')
            script = (ROOT / 'deploy/scripts/restore-drill.sh').read_text()
            snippet = re.search(r'<<\'PY\'\n(import hashlib,json,pathlib,re,subprocess,sys.*?)\nPY', script, re.S).group(1)
            # The helper is passed explicitly; an unrelated cwd cannot choose imported code.
            with patch('sys.argv', ['drill', str(root), str(ROOT / 'deploy/lib/blob_integrity.py')]), patch('subprocess.check_output', side_effect=query), \
                 patch('pathlib.Path.cwd', return_value=root):
                with self.assertRaises(ValueError): exec(compile(snippet, 'drill-comparison', 'exec'), {})

    def test_restored_main_and_thumbnail_missing_size_corruption_and_symlink(self):
        (ROOT / 'var/tests').mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=ROOT / 'var/tests') as tmp:
            root = Path(tmp)
            values = [b'main', b'thumb']
            hashes = [hashlib.sha256(value).hexdigest() for value in values]
            rows = [dict(household_id='h', sha256=sha, size=len(value)) for sha, value in zip(hashes, values)]
            refs = [dict(household_id='h', blob_sha256=hashes[0], thumbnail_sha256=hashes[1])]
            paths = [root / sha[:2] / sha[2:4] / sha for sha in hashes]
            for path, value in zip(paths, values): path.parent.mkdir(parents=True); path.write_bytes(value)
            def query(sql): return json.dumps(rows if 'to_jsonb(b)' in sql else refs)
            verify = lambda: backup.blob_integrity.verify_restored(query, root)
            verify()  # Good main + thumbnail pass, even without a manifest inventory.
            thumb = paths[1]
            thumb.unlink()
            with self.assertRaises(FileNotFoundError): verify()
            thumb.write_bytes(b'wrong')
            with self.assertRaises(ValueError): verify()
            thumb.write_bytes(values[1]); rows[1]['size'] += 1
            with self.assertRaises(ValueError): verify()
            rows[1]['size'] -= 1
            thumb.unlink(); thumb.symlink_to(paths[0])
            with self.assertRaises(ValueError): verify()
            thumb.unlink(); thumb.write_bytes(values[1]); rows.pop()
            with self.assertRaises(ValueError): verify()

if __name__ == '__main__': unittest.main()
