#!/usr/bin/env bash
# Sourceable, offline LAN discovery for development only; never used by production deployment.
# Overrides: DEV_LAN_IP and DEV_LAN_CIDR. No filesystem or Docker side effects.
dev_lan_resolve() {
  local resolved
  resolved=$(python3 -I - "${DEV_LAN_IP:-}" "${DEV_LAN_CIDR:-}" "${DEV_LAN_SYSFS_ROOT:-/sys}" <<'PY'
import ipaddress
from pathlib import Path
import subprocess
import sys


def ip_output(*args, optional=False):
    try:
        result = subprocess.run(['ip', '-4', '-o', *args], capture_output=True, text=True)
    except OSError as error:
        raise ValueError('cannot discover IPv4 LAN; set DEV_LAN_IP and DEV_LAN_CIDR') from error
    if result.returncode:
        if optional:
            return ''
        raise ValueError('cannot discover IPv4 LAN; set DEV_LAN_IP and DEV_LAN_CIDR')
    return result.stdout


def field(fields, name, default=''):
    return fields[fields.index(name) + 1] if name in fields else default


try:
    address, cidr, sysfs = sys.argv[1:]
    device = ''
    fallback = False
    global_addresses = None

    def physical(name):
        return bool(name) and (Path(sysfs) / 'class/net' / name / 'device').exists()

    def addresses():
        global global_addresses
        if global_addresses is None:
            global_addresses = []
            for line in ip_output('addr', 'show', 'scope', 'global').splitlines():
                fields = line.split()
                if 'inet' in fields:
                    global_addresses.append((fields[1], ipaddress.IPv4Interface(field(fields, 'inet')),
                                             'secondary' in fields))
        return global_addresses

    if not address:
        candidates = []
        for line in ip_output('route', 'show', 'table', 'main', 'default', optional=True).splitlines():
            fields = line.split()
            if fields and fields[0] == 'default' and physical(field(fields, 'dev')):
                candidates.append((int(field(fields, 'metric', '0')), fields))
        if candidates:
            # min is stable: missing metric = 0, ties keep the first listed route.
            fields = min(candidates, key=lambda candidate: candidate[0])[1]
            device = field(fields, 'dev')
            address = field(fields, 'src')
            if not address:
                primary = [interface for name, interface, secondary in addresses()
                           if name == device and not secondary]
                if not primary:
                    raise ValueError('no primary global IPv4 address; set DEV_LAN_IP and DEV_LAN_CIDR')
                address = str(primary[0].ip)
        else:
            fallback = True
            fields = ip_output('route', 'get', '1.1.1.1').split()
            device = field(fields, 'dev')
            sources = {fields[i + 1] for i, value in enumerate(fields[:-1]) if value == 'src'}
            if len(sources) != 1:
                raise ValueError('no unique IPv4 source; set DEV_LAN_IP and DEV_LAN_CIDR')
            address = sources.pop()
    ip = ipaddress.IPv4Address(address)
    if ip.is_unspecified or ip.is_loopback or ip.is_link_local or ip.is_multicast:
        raise ValueError('source is not a LAN IPv4 address; set DEV_LAN_IP')
    if not cidr or (fallback and not physical(device)):
        matching = [interface for name, interface, _ in addresses()
                    if interface.ip == ip and (not device or name == device)]
        if fallback and not physical(device) and any(interface.network.prefixlen == 32 for interface in matching):
            raise ValueError('tunnel-like IPv4 source; set DEV_LAN_IP and DEV_LAN_CIDR')
        if not cidr:
            networks = {str(interface.network) for interface in matching}
            if len(networks) != 1:
                raise ValueError('no unique subnet for IPv4 source; set DEV_LAN_CIDR')
            cidr = networks.pop()
    if '/' not in cidr:
        raise ValueError('DEV_LAN_CIDR must include a prefix length')
    network = ipaddress.IPv4Network(cidr, strict=False)
    if ip not in network:
        raise ValueError('DEV_LAN_IP is outside DEV_LAN_CIDR')
    print(ip, network)
except (ValueError, IndexError, OSError) as error:
    print('dev-lan: ' + str(error), file=sys.stderr)
    sys.exit(1)
PY
  ) || return 1
  read -r DEV_LAN_IP DEV_LAN_CIDR <<< "$resolved"
  export DEV_LAN_IP DEV_LAN_CIDR
}

dev_lan_config() {
  dev_lan_resolve || return 1
  export ENROLL_CIDRS=${ENROLL_CIDRS:-$DEV_LAN_CIDR,${BACKEND_GATEWAY:-172.31.250.1}/32,${INGRESS_GATEWAY:-172.31.251.1}/32}
  export PORCHLIGHT_ENDPOINTS=${PORCHLIGHT_ENDPOINTS:-https://localhost:${LAN_PORT:-8443},https://10.0.2.2:${LAN_PORT:-8443},https://$DEV_LAN_IP:${LAN_PORT:-8443}}
}
