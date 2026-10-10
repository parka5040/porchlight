#!/usr/bin/env python3
"""Independent restore TLS/application check. Public certs only, no auth secrets."""
import argparse
import hashlib
import http.client
import json
from pathlib import Path
import signal
import socket
import ssl
import sys


def probe(host, port, ca, server_cert, version):
    context = ssl.create_default_context(cafile=str(ca))
    # Restore drill uses a loopback port, not the original server's DNS name.
    # CA validation + exact restored leaf pin authenticate it independently of DNS.
    context.check_hostname = False
    expected = hashlib.sha256(ssl.PEM_cert_to_DER_cert(Path(server_cert).read_text())).digest()
    with socket.create_connection((host, port), timeout=4) as tcp:
        with context.wrap_socket(tcp, server_hostname=host) as tls:
            if hashlib.sha256(tls.getpeercert(binary_form=True)).digest() != expected:
                raise ValueError('TLS leaf differs from restored server certificate')
            tls.sendall(('GET /v1/enrollment/complete HTTP/1.1\r\nHost: localhost\r\n'
                         'Porchlight-Client-Version: '+version+'\r\nConnection: close\r\n\r\n').encode('ascii'))
            response = http.client.HTTPResponse(tls)
            response.begin()
            body = response.read(4097)
            if len(body) > 4096: raise ValueError('oversized readiness response')
            problem = json.loads(body)
            # Existing POST route, deliberately safe wrong-method GET: real router response.
            if response.status != 405 or problem.get('status') != 405 or problem.get('type') != 'urn:porchlight:problem:method-not-allowed':
                raise ValueError('unexpected application response')


def deadline(*_): raise TimeoutError('TLS readiness deadline exceeded')

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--host', default='127.0.0.1')
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--ca', type=Path, required=True)
    parser.add_argument('--server-cert', type=Path, required=True)
    parser.add_argument('--version', required=True)
    args = parser.parse_args()
    signal.signal(signal.SIGALRM, deadline)
    signal.alarm(5)  # Whole probe, including a peer trickling body bytes.
    try:
        probe(args.host, args.port, args.ca, args.server_cert, args.version)
    except Exception:
        print('FAIL TLS/application readiness', file=sys.stderr)
        sys.exit(1)
    finally:
        signal.alarm(0)
    print('PASS TLS/application readiness')
