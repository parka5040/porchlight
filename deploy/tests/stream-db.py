"""Exercise the gate's actual SQL helper; never invoke TLS fixtures or print credentials."""
import os, pathlib, shutil
root = pathlib.Path(__file__).resolve().parents[2]
source = (root / 'deploy/scripts/m5-demo-stream.sh').read_text().split("<<'PY'\n", 1)[1].rsplit('\nPY', 1)[0]
os.environ['M5_STREAM_ROOT'] = str(root)
os.environ['M5_STREAM_DATABASE_URL'] = os.environ['PORCHLIGHT_SUPERUSER_URL']
os.environ['PGHOST'] = str(root / 'var/pgsock')
os.environ['PGUSER'] = 'postgres'
os.environ['PGPASSWORD'] = os.environ['POSTGRES_PASSWORD']
namespace = {}
try:
    exec(compile(source.split('rc = 0', 1)[0], '<stream-demo-helper>', 'exec'), namespace)
    assert namespace['sql']('SELECT 1') == '1'
    service = namespace['work'] / 'pg_service.conf'
    assert service.stat().st_mode & 0o777 == 0o600
    print('PASS stream demo SQL URL uses private service file')
    from unittest.mock import patch
    import subprocess
    namespace['base'] = 'https://example.invalid'
    credentials = namespace['work'] / 'client'
    credentials.mkdir()
    (credentials / 'session.token').write_text('fixture')
    with patch('subprocess.run', side_effect=subprocess.TimeoutExpired('curl', 20)):
        try:
            namespace['request'](credentials, 'GET', '/v1/me')
        except subprocess.TimeoutExpired:
            pass
        else:
            raise AssertionError('missing expected timeout')
    assert not list(namespace['work'].glob('*.curl'))
    print('PASS stream demo timeout removes bearer curl config')
finally:
    if 'log' in namespace: namespace['log'].close()
    if 'work' in namespace: shutil.rmtree(namespace['work'])
