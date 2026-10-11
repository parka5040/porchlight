#!/usr/bin/env python3
"""Offline release artifact/dependency regressions; run after assembleRelease."""
from pathlib import Path
import unittest
from zipfile import ZipFile

ROOT = Path(__file__).resolve().parents[2]
CLIENT = ROOT / "client"


class AuditApkTest(unittest.TestCase):
    def test_release_is_shrunk(self):
        output = CLIENT / "androidApp/build/outputs"
        self.assertTrue((output / "mapping/release/usage.txt").is_file(),
                        "F1-01: release must run R8 and emit its removal report")
        self.assertTrue((output / "mapping/release/resources.txt").is_file(),
                        "F1-01: release must emit the resource shrinking report")
        seeds = (output / "mapping/release/seeds.txt").read_text()
        for worker in ("HouseholdSyncWorker", "NotificationActionWorker"):
            self.assertIn(f"app.porchlight.shared.live.{worker}: {worker}(android.content.Context,androidx.work.WorkerParameters)", seeds)
        apks = list((output / "apk/release").glob("*.apk"))
        self.assertEqual(len(apks), 1, "Build exactly one release APK before this check")
        with ZipFile(apks[0]) as apk:
            dex_size = sum(entry.file_size for entry in apk.infolist() if entry.filename.endswith(".dex"))
            # The unshrunk baseline is 50.4 MB dex. Leave growth room, but catch disabled R8.
            self.assertLess(dex_size, 35_000_000, "F1-01: release dex regressed to unshrunk size")
            resources = "assets/composeResources/app.porchlight.shared.resources/"
            for font in ("literata_semibold", "source_sans_3_regular", "source_sans_3_semibold", "source_sans_3_bold"):
                self.assertIn(resources + "font/" + font + ".ttf", apk.namelist())
            self.assertIn(resources + "drawable/porchlight_icon.png", apk.namelist())
            for notice in ("META-INF/LICENSE.md", "META-INF/NOTICE.md", resources + "font/OFL-literata.txt",
                           resources + "font/OFL-source-sans-3.txt", "assets/PublicSuffixDatabase.list"):
                self.assertIn(notice, apk.namelist())
            dex = b"".join(apk.read(entry) for entry in apk.namelist() if entry.endswith(".dex"))
            for test_only in (b"AuditApkProbeActivity", b"ShellFixtureActivity"):
                self.assertFalse(test_only in dex, "Test-only activity leaked into release dex")

    def test_runtime_dependencies_exclude_test_only_libraries(self):
        forbidden = ("ktor-client-content-negotiation", "ktor-serialization-kotlinx-json", "coroutines-extensions")
        for module, runtime in (("shared", "androidRuntimeClasspath"), ("shared", "jvmRuntimeClasspath"),
                                ("androidApp", "releaseRuntimeClasspath"), ("desktopApp", "runtimeClasspath")):
            for line in (CLIENT / module / "gradle.lockfile").read_text().splitlines():
                if "=" not in line:
                    continue
                dependency, configurations = line.split("=", 1)
                if any(name in dependency for name in forbidden):
                    self.assertNotIn(runtime, configurations.split(","),
                                     f"F1-05: {module} {runtime} retains {dependency}")


if __name__ == "__main__":
    unittest.main()
