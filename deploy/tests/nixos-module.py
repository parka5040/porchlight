#!/usr/bin/env python3
"""Offline host contract check; controller must still evaluate the module with Nix."""
from pathlib import Path
import re
import sys


REQUIRED = (
    'cfg = config.porchlight.host;',
    'user = lib.mkOption {',
    'dataMount = lib.mkOption {',
    'users.groups.porchlight = { };',
    'users.users.${cfg.user}.extraGroups = [ "porchlight" ];',
    'environment.systemPackages = with pkgs; [ minisign age ];',
    '"d /run/porchlight 0750 ${cfg.user} porchlight -"',
    '"d ${cfg.dataMount}/porchlight 0750 ${cfg.user} porchlight -"',
    'systemd.services.docker.unitConfig.RequiresMountsFor = [ cfg.dataMount ];',
    'networking.firewall.allowedTCPPorts = [ 8443 8444 ];',
)


def strip_comments_and_check_balance(source: str) -> str:
    """Ignore # comments outside strings and check delimiters outside strings."""
    result = []
    stack = []
    quoted = escaped = comment = False
    pairs = {"}": "{", "]": "[", ")": "("}
    for char in source:
        if comment:
            if char == "\n":
                comment = False
                result.append(char)
            continue
        if quoted:
            result.append(char)
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quoted = False
            continue
        if char == "#":
            comment = True
            continue
        result.append(char)
        if char == '"':
            quoted = True
        elif char in "{[(":
            stack.append(char)
        elif char in "}])":
            assert stack and stack.pop() == pairs[char], "unbalanced braces/brackets"
    assert not quoted, "unbalanced quotes"
    assert not stack, "unbalanced braces/brackets"
    return "".join(result)


def validate(source: str) -> None:
    code = strip_comments_and_check_balance(source)
    compact = " ".join(code.split())
    for fragment in REQUIRED:
        assert fragment in compact, f"missing required fragment: {fragment}"
    # Scan every firewall ports assignment, not merely the required LAN assignment.
    for ports in re.findall(r"networking\.firewall\.[\w.]*Ports\s*=\s*\[([^]]*)\]", code):
        assert not re.search(r"\b(?:9443|5432)\b", ports), "private port in firewall"
    assert not re.search(r"\bsudo\b", code), "sudo is not a module prerequisite"
    assert not re.search(r"\b(?:secrets?|authKey|password|token|privateKey)\b", code, re.I), "secret material/configuration in module"
    assert not re.search(r"\bservices\s*\.\s*tailscale\b", code), "host Tailscale must remain unchanged"
    # The account and mount are the owner's machine values: set in their NixOS config, never defaulted here.
    assert not re.search(r"\bdefault\s*=", code), "machine-specific options must not have defaults"


def main() -> None:
    module = Path(__file__).resolve().parents[1] / "nixos-porchlight.nix"
    try:
        validate(module.read_text())
    except (OSError, AssertionError) as error:
        sys.exit(f"FAIL nixos-module: {error}")
    print("PASS nixos-module")


if __name__ == "__main__":
    main()
