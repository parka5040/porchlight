#!/usr/bin/env python3
"""Exercise the packaged POSIX launcher without Java, a display or external WM tools."""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "client/desktopApp/launcher/porchlight.sh"


class DesktopLauncherTest(unittest.TestCase):
    def test_environment_decision_and_exec_contract(self):
        self.assertTrue(SOURCE.is_file(), "packaged Wayland launcher wrapper is missing")
        runtime = ROOT / "var"
        runtime.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="launcher space-", dir=runtime) as temporary:
            binary = Path(temporary) / "bin"
            binary.mkdir()
            wrapper = binary / "porchlight"
            shutil.copyfile(SOURCE, wrapper)
            wrapper.chmod(0o755)
            native = binary / "porchlight-app"
            native.write_text('''#!/bin/sh
printf '%s\\n' "${_JAVA_AWT_WM_NONREPARENTING+x}" "${_JAVA_AWT_WM_NONREPARENTING-unset}"
printf '%s\\n' "$#"
for argument do printf '<%s>\\n' "$argument"; done
IFS= read -r input
printf 'stdin:<%s>\\n' "$input"
printf 'native stderr\\n' >&2
exit 37
''')
            native.chmod(0o755)
            subprocess.run(["/bin/sh", "-n", str(wrapper)], check=True)
            cases = [
                ("niri", {"WAYLAND_DISPLAY": "wayland-1", "XDG_CURRENT_DESKTOP": "niri"}, "1"),
                ("sway", {"WAYLAND_DISPLAY": "wayland-1", "XDG_CURRENT_DESKTOP": "sway"}, "1"),
                ("unknown", {"XDG_SESSION_TYPE": "wayland"}, "1"),
                ("gnome", {"WAYLAND_DISPLAY": "wayland-1", "XDG_CURRENT_DESKTOP": "ubuntu:GNOME"}, None),
                ("kde", {"XDG_SESSION_TYPE": "wayland", "XDG_CURRENT_DESKTOP": "KDE"}, None),
                ("x11", {"DISPLAY": ":0", "XDG_SESSION_TYPE": "x11"}, None),
                ("force on X11", {"DISPLAY": ":0", "PORCHLIGHT_AWT_NONREPARENTING": "1"}, "1"),
                ("force off", {"WAYLAND_DISPLAY": "wayland-1", "PORCHLIGHT_AWT_NONREPARENTING": "0"}, None),
                ("invalid override", {"WAYLAND_DISPLAY": "wayland-1", "PORCHLIGHT_AWT_NONREPARENTING": "other"}, "1"),
                ("preset zero", {"WAYLAND_DISPLAY": "wayland-1", "PORCHLIGHT_AWT_NONREPARENTING": "1", "_JAVA_AWT_WM_NONREPARENTING": "0"}, "0"),
                ("preset empty", {"WAYLAND_DISPLAY": "wayland-1", "_JAVA_AWT_WM_NONREPARENTING": ""}, ""),
                ("preset arbitrary", {"PORCHLIGHT_AWT_NONREPARENTING": "0", "_JAVA_AWT_WM_NONREPARENTING": "custom"}, "custom"),
                ("not substring", {"WAYLAND_DISPLAY": "wayland-1", "XDG_CURRENT_DESKTOP": "NotGNOME"}, "1"),
            ]
            for desktop in ("GNOME", "KDE", "Cinnamon", "X-Cinnamon", "Pantheon", "Unity"):
                cases.append((desktop, {"WAYLAND_DISPLAY": "wayland-1", "XDG_CURRENT_DESKTOP": "niri:" + desktop.swapcase() + ":other"}, None))
            for desktop in ("Hyprland", "river", "labwc", "Wayfire"):
                cases.append((desktop, {"WAYLAND_DISPLAY": "wayland-1", "XDG_CURRENT_DESKTOP": desktop}, "1"))
            for name, environment, expected in cases:
                with self.subTest(name=name):
                    # No inherited session state or PATH utilities can influence the decision.
                    result = subprocess.run([str(wrapper), "--tray", "argument with spaces", "", "*literal"],
                        env={"PATH": "/nonexistent", **environment}, input="from stdin\n", text=True, capture_output=True)
                    self.assertEqual(37, result.returncode)
                    self.assertEqual("native stderr\n", result.stderr)
                    self.assertEqual(["x" if expected is not None else "", expected if expected is not None else "unset",
                        "4", "<--tray>", "<argument with spaces>", "<>", "<*literal>", "stdin:<from stdin>"], result.stdout.splitlines())


if __name__ == "__main__":
    unittest.main()
