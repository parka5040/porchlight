#!/usr/bin/env python3
"""Exercise the live demo's ERR trap without credentials or a running stack."""
from pathlib import Path
import subprocess
import sys

lines = Path(sys.argv[1]).read_text().splitlines()
flags = next(line for line in lines if line.startswith("set -"))
trap = next(line for line in lines if line.startswith("trap ") and line.endswith(" ERR"))
probe = flags + "\n" + trap + "\nf() { false; }; f\n"
result = subprocess.run(["bash", "-c", probe], capture_output=True, text=True)
assert result.returncode == 1
assert "FAIL lists gate (line " in result.stderr
print("lists demo nested ERR trap: OK")
