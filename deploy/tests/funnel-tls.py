#!/usr/bin/env python3
"""Offline full-controller TLS regressions: real loopback TLS, fake SSH/DNS only."""
import json
import os
import pathlib
import shutil
import socket
import ssl
import subprocess
import sys
import tempfile
import threading

fixture = pathlib.Path(sys.argv[1])
fqdn = 'quiet-node.example.ts.net'
stack = fixture / 'remote/stack'
log = fixture / 'var/shims.jsonl'

# Run the actual embedded probe, only substituting DNS and shortening its test timeout.
# Every other Python invocation (including remote checks) uses unmodified isolated Python.
(fixture / 'bin/python3').write_text('''#!/usr/bin/python3 -I
import json,os,socket,sys
args=sys.argv[1:]
if len(args)>2 and args[:2]==['-I','-c'] and '# funnel-public-tls' in args[2]:
 with open(os.environ['SHIM_LOG'],'a') as f:
  f.write(json.dumps({'tool':'python-probe','isolated':True,'stdin_bytes':len(sys.stdin.buffer.read())})+'\\n')
 mode=os.getenv('PROBE_PATH','public')
 def lookup(host,port,*args,**kwargs):
  assert host=='quiet-node.example.ts.net' and port==443
  if mode=='dns-error': raise socket.gaierror('controlled fixture error')
  public=(socket.AF_INET,socket.SOCK_STREAM,6,'',('127.0.0.1',int(os.environ['TLS_PORT'])))
  v4=(socket.AF_INET,socket.SOCK_STREAM,6,'',('100.64.0.1',443))
  v6=(socket.AF_INET6,socket.SOCK_STREAM,6,'',('fd7a:115c:a1e0::1',443,0,0))
  return {'public':[public],'v4':[v4],'v6':[v6],'mixed':[public,v4]}.get(mode,[public])
 socket.getaddrinfo=lookup
 sys.argv=['-c']+args[3:]
 exec(args[2].replace('budget=20','budget=0.35'),{'__name__':'__main__'})
else: os.execv('/usr/bin/python3',['python3']+args)
''')
(fixture / 'bin/python3').chmod(0o755)
# The pre-fix script must not invoke a real network OpenSSL client in the RED run.
(fixture / 'bin/openssl').write_text('''#!/bin/bash
printf 'Verification: OK\\ntlsv13 alert certificate required\\n'
exit 1
''')
(fixture / 'bin/openssl').chmod(0o755)


def run(expected=0, *, env=None, args=None):
    log.write_text('')
    result = subprocess.run(
        ['bash', str(fixture / 'scripts/deploy-server.sh'), 'funnel-check'] + (args or []),
        env=dict(os.environ, **(env or {})), stdin=subprocess.DEVNULL,
        capture_output=True, text=True, timeout=10)
    (fixture / 'var/stdout').write_text(result.stdout)
    (fixture / 'var/stderr').write_text(result.stderr)
    assert result.returncode == expected, (expected, result.returncode, result.stdout, result.stderr)
    for text in [result.stdout, result.stderr, log.read_text()]:
        for secret in ['ENV-SECRET-FIXTURE', 'TS-KEY-STDIN-ONLY', 'SECRET-ENROLLMENT-FIXTURE']:
            assert secret not in text, 'secret escaped private fixture'
    return result.stdout, result.stderr


class Server:
    def __init__(self, cert, key, ca, *, required=True, stall=False, eof=False):
        self.context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        self.context.minimum_version = ssl.TLSVersion.TLSv1_3
        self.context.load_cert_chain(cert, key)
        self.context.load_verify_locations(ca)
        self.context.verify_mode = ssl.CERT_REQUIRED if required else ssl.CERT_NONE
        self.stall, self.eof = stall, eof
        self.listener = socket.socket()
        self.listener.bind(('127.0.0.1', 0))
        self.listener.listen()
        self.listener.settimeout(0.1)
        self.port = self.listener.getsockname()[1]
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.serve, daemon=True)

    def serve(self):
        while not self.stop.is_set():
            try:
                conn, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            with conn:
                conn.settimeout(1)
                if self.stall:
                    self.stop.wait(1)
                    continue
                try:
                    with self.context.wrap_socket(conn, server_side=True) as tls:
                        if not self.eof:
                            tls.sendall(b'not mandatory mtls')
                except (ssl.SSLError, OSError):
                    pass  # Expected empty client certificate or client verification rejection.

    def __enter__(self):
        self.thread.start()
        os.environ['TLS_PORT'] = str(self.port)
        return self

    def __exit__(self, *_):
        self.stop.set()
        self.listener.close()
        self.thread.join(timeout=2)
        assert not self.thread.is_alive()


def openssl(*args):
    subprocess.run(['/usr/bin/openssl', *map(str, args)], check=True,
                   stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


with tempfile.TemporaryDirectory(prefix='funnel-tls.', dir=fixture / 'var') as temp:
    temp = pathlib.Path(temp)
    ca, cakey = temp / 'ca.crt', temp / 'ca.key'
    openssl('req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
            '-subj', '/CN=offline fixture CA', '-addext', 'basicConstraints=critical,CA:TRUE',
            '-addext', 'keyUsage=critical,keyCertSign,cRLSign', '-keyout', cakey, '-out', ca)

    def leaf(name, hostname, subject, *, self_signed=False):
        key, cert, csr = (temp / (name + ext) for ext in ['.key', '.crt', '.csr'])
        args = ['req', '-newkey', 'rsa:2048', '-nodes', '-subj', '/CN=' + subject,
                '-keyout', key, '-addext', 'subjectAltName=DNS:' + hostname]
        if self_signed:
            openssl(*args, '-x509', '-days', '1', '-out', cert)
        else:
            openssl(*args, '-out', csr)
            openssl('x509', '-req', '-in', csr, '-CA', ca, '-CAkey', cakey,
                    '-CAcreateserial', '-days', '1', '-copy_extensions', 'copy', '-out', cert)
        return cert, key

    good = leaf('good', fqdn, 'offline server')
    attacker = leaf('attacker', fqdn, 'Verification: OK tlsv13 alert certificate required', self_signed=True)
    wrong = leaf('wrong', 'other.example.ts.net', 'offline wrong name')
    shutil.copyfile(ca, stack / 'data/pki/ca.crt')
    (stack / 'tailscale/state').mkdir(parents=True, exist_ok=True)
    (stack / 'tailscale').chmod(0o700)
    (stack / 'tailscale/state').chmod(0o700)
    (stack / 'tailscale/serve.json').write_text('{}\n')
    (stack / 'tailscale/serve.json').chmod(0o600)
    envpath = stack / '.env'
    cleanenv = ('FUNNEL_ENABLED=1\nTS_AUTHKEY_SPEC=\nPOSTGRES_PASSWORD=ENV-SECRET-FIXTURE\n'
                'PORCHLIGHT_ENDPOINTS=https://porchlight.lan:8443,https://' + fqdn + '\n')
    envpath.write_text(cleanenv)
    envpath.chmod(0o600)

    for certkey, options, expected in [
        (good, {}, 0), (good, {'required': False}, 1),
        (attacker, {}, 1), (wrong, {}, 1),
        (good, {'stall': True}, 1), (good, {'required': False, 'eof': True}, 1),
    ]:
        with Server(*certkey, ca, **options):
            stdout, stderr = run(expected)
            assert ('PASS public-tls' if expected == 0 else 'FAIL public-tls') in stdout
            assert 'Verification: OK' not in stdout + stderr
    print('PASS real TLS 1.3: household CA + required client cert; reject optional mTLS, subject spoof, wrong SAN, timeout and EOF')

    with Server(*good, ca):
        stdout, _ = run()
        lines = stdout.splitlines()
        assert len(lines) == 12 and len(set(lines)) == 12 and all(s.startswith('PASS ') for s in lines), lines
        rows = [json.loads(s) for s in log.read_text().splitlines()]
        probe = next(r for r in rows if r['tool'] == 'python-probe')
        assert probe['isolated'] and probe['stdin_bytes'] == 0
        assert (fixture / 'var/m9-funnel/ca.crt').read_bytes() == ca.read_bytes()
        assert (fixture / 'var/m9-funnel').stat().st_mode & 0o777 == 0o700
        assert (fixture / 'var/m9-funnel/ca.crt').stat().st_mode & 0o777 == 0o600
        for mode in ['v4', 'v6', 'mixed', 'dns-error']:
            stdout, stderr = run(1, env={'PROBE_PATH': mode})
            assert 'FAIL probe-path' in stdout and 'PASS public-tls' not in stdout
            if mode != 'dns-error':
                assert 'this machine is on the tailnet; run funnel-check off-tailnet' in stderr
        print('PASS off-tailnet probe-path rejects IPv4, IPv6, mixed DNS answers and resolution failure')
        for fault, check in [('running', 'tailscale-running'), ('authkey', 'authkey-cleared'),
                             ('serve', 'raw-tcp-proxy-v2'), ('proxy', 'raw-tcp-proxy-v2'),
                             ('allow', 'raw-tcp-proxy-v2'), ('sans', 'production-sans')]:
            stdout, _ = run(1, env={'FUNNEL_BAD': fault})
            assert 'FAIL ' + check in stdout and 'FAIL remote-status' in stdout
            assert 'status-transport' not in stdout
        stdout, _ = run(1, env={'STATUS_SSH_FAIL': '1'})
        assert 'FAIL status-transport' in stdout and 'FAIL remote-status' not in stdout
        for path in [stack / 'tmp/m9-ts-authkey', stack / 'tailscale/authkey']:
            for symlink in [False, True]:
                if symlink:
                    path.symlink_to('missing-target')
                else:
                    path.write_text('TS-KEY-STDIN-ONLY')
                try:
                    stdout, _ = run(1)
                    assert 'FAIL leftover-authkey' in stdout
                finally:
                    path.unlink()
        for edit in ['TS_AUTHKEY_SPEC=file:/config/authkey\n', 'PORCHLIGHT_ENDPOINTS=https://wrong.example\n',
                     'FUNNEL_ENABLED=0\n']:
            envpath.write_text(cleanenv + edit)
            try:
                stdout, _ = run(1)
                assert 'FAIL private-state' in stdout
            finally:
                envpath.write_text(cleanenv)
        (stack / 'tailscale/serve.json').chmod(0o644)
        stdout, _ = run(1)
        assert 'FAIL private-state' in stdout
        (stack / 'tailscale/serve.json').chmod(0o600)
        for name in ['bad;name', 'not-a-fqdn', '-bad.example.ts.net', '1.2.3.4', 'porchlight.lan', 'ts.net']:
            run(2, args=['--fqdn', name])
            assert not log.read_text()
        saved = fixture / 'var/m9/funnel-fqdn'
        saved.write_text('porchlight.lan\n')
        run(2)
        assert not log.read_text()
        saved.write_text(fqdn + '\n')
    print('PASS precise remote-status/transport, leftover-authkey, private-state, SAN and ts.net checks; no .env secret output')
# TemporaryDirectory removes every generated test certificate and key on all exits.
