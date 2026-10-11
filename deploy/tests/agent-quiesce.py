"""Run the installer's real agent quiesce/resume SQL against a private throwaway PostgreSQL cluster.

The pre-upgrade quiesce must terminate only porchlight_agent sessions: never its own session (psql would fail
and every upgrade would roll back) nor any other role's. Uses host initdb/pg_ctl on a unix socket only.
"""
import os, pathlib, re, shutil, subprocess, sys, tempfile, time

root = pathlib.Path(__file__).resolve().parents[2]
installer = (root / 'deploy/install.sh').read_text()
functions = ''.join(
    re.search(rf'^{name}\(\) \{{\n.*?^\}}\n', installer, re.S | re.M).group(0)
    for name in ('quiesce_agent', 'resume_agent'))

work = pathlib.Path(tempfile.mkdtemp(prefix='pgq'))
data, sock = work / 'data', work / 's'
sock.mkdir()
env = {**os.environ, 'PGHOST': str(sock), 'PGDATABASE': 'postgres'}
for name in ('PGPASSWORD', 'PGSERVICE', 'PGSERVICEFILE', 'PGPASSFILE', 'DATABASE_URL', 'PGUSER', 'PGPORT'):
    env.pop(name, None)
sessions = []


def run(*args, **kw):
    return subprocess.run(args, env=env, check=True, capture_output=True, text=True, timeout=60, **kw)


def sql(query, user='postgres'):
    return run('psql', '-X', '-qtA', '-U', user, '-v', 'ON_ERROR_STOP=1', '-c', query).stdout.strip()


def installer_step(name):
    # The real function bodies, with `dc exec -T postgres psql ...` routed to the private cluster.
    script = ('set -euo pipefail\n'
              'dc() { [[ $1 == exec && $2 == -T && $3 == postgres && $4 == psql ]] || exit 9; shift 4; psql "$@"; }\n'
              f'agent_gate_owned=0\n{functions}{name}\n')
    return subprocess.run(['bash', '-c', script], env=env, capture_output=True, text=True, timeout=60)


def sleeper(user):
    proc = subprocess.Popen(['psql', '-X', '-q', '-U', user, '-c', 'SELECT pg_sleep(120)'], env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    sessions.append(proc)
    return proc


def active(user):
    return int(sql(f"SELECT count(*) FROM pg_stat_activity WHERE usename = '{user}'"))


try:
    run('initdb', '-D', str(data), '-U', 'postgres', '--auth=trust', '--locale=C', '-E', 'UTF8', '--no-sync')
    run('pg_ctl', '-D', str(data), '-s', '-w', '-l', str(work / 'server.log'),
        '-o', f"-k {sock} -c listen_addresses='' -c fsync=off", 'start')
    sql('CREATE ROLE porchlight_agent LOGIN; CREATE ROLE quiesce_bystander LOGIN')
    agent, bystander = sleeper('porchlight_agent'), sleeper('quiesce_bystander')
    deadline = time.monotonic() + 20
    while not (active('porchlight_agent') and active('quiesce_bystander')):
        assert time.monotonic() < deadline, 'probe sessions did not connect'
        time.sleep(0.1)

    quiesced = installer_step('quiesce_agent')
    assert quiesced.returncode == 0, f'quiesce_agent failed ({quiesced.returncode}): {quiesced.stderr.strip()}'
    assert agent.wait(timeout=20) != 0, 'agent session survived quiesce'
    assert active('porchlight_agent') == 0
    assert bystander.poll() is None and active('quiesce_bystander') == 1, 'quiesce terminated a non-agent session'
    assert sql("SELECT rolcanlogin FROM pg_roles WHERE rolname = 'porchlight_agent'") == 'f'
    print('PASS agent quiesce terminates only agent sessions and keeps its own')

    resumed = installer_step('resume_agent')
    assert resumed.returncode == 0, f'resume_agent failed ({resumed.returncode}): {resumed.stderr.strip()}'
    print('PASS resume without an owned gate is a no-op')
    script_owned = installer_step('agent_gate_owned=1; resume_agent')
    assert script_owned.returncode == 0, script_owned.stderr.strip()
    assert sql("SELECT rolcanlogin FROM pg_roles WHERE rolname = 'porchlight_agent'") == 't'
    print('PASS agent resume restores LOGIN')
finally:
    for proc in sessions:
        if proc.poll() is None:
            proc.kill()
        proc.wait()
    if (data / 'postmaster.pid').exists():
        subprocess.run(['pg_ctl', '-D', str(data), '-s', '-m', 'immediate', 'stop'], env=env,
                       capture_output=True, timeout=60)
    shutil.rmtree(work, ignore_errors=True)
