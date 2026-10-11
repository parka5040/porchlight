#!/usr/bin/env python3
"""Check the shipping desktop image, not Gradle's unmodified dependency cache."""
from pathlib import Path
import argparse
import os
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[2]
NATIVES = "org/sqlite/native/"
LINUX_X64 = NATIVES + "Linux/x86_64/libsqlitejdbc.so"
IMAGE = ROOT / "client/desktopApp/build/compose/binaries/main/app/porchlight"


class AuditDesktopPackagingTest(unittest.TestCase):
    def sqlite(self):
        jars = list((IMAGE / "lib/app").glob("sqlite-jdbc-*.jar"))
        self.assertEqual(1, len(jars), "shipping image must contain exactly one SQLite driver")
        return jars[0]

    def test_only_linux_x64_sqlite_native_is_packaged(self):
        with zipfile.ZipFile(self.sqlite()) as jar:
            natives = [i.filename for i in jar.infolist() if i.filename.startswith(NATIVES) and not i.is_dir()]
        self.assertEqual([LINUX_X64], natives, "shipping SQLite jar includes non-Linux-x64 natives")

    def test_sqlite_classes_services_and_notices_are_unchanged(self):
        packaged = self.sqlite()
        cache = Path(os.environ.get("GRADLE_USER_HOME", ROOT / "var/gradle-home"))
        sources = [p for p in (cache / "caches/modules-2/files-2.1/org.xerial/sqlite-jdbc").glob("*/*/sqlite-jdbc-*.jar")
                   if packaged.name == p.name or packaged.name.startswith(p.stem + "-")]
        self.assertEqual(1, len(sources), "expected the resolved upstream SQLite jar in the offline cache")
        with zipfile.ZipFile(sources[0]) as source, zipfile.ZipFile(packaged) as target:
            expected = {i.filename: source.read(i) for i in source.infolist()
                        if not i.is_dir() and (not i.filename.startswith(NATIVES) or i.filename == LINUX_X64)}
            actual = {i.filename: target.read(i) for i in target.infolist() if not i.is_dir()}
            self.assertEqual(set(expected), set(actual), "only foreign natives may be removed")
            for name, content in expected.items():
                self.assertEqual(content, actual[name], "changed SQLite entry: " + name)
            self.assertIn(b"org.sqlite.JDBC", actual["META-INF/services/java.sql.Driver"])
            entries = target.infolist()
            self.assertEqual(len(entries), len({i.filename for i in entries}), "duplicate zip entries")
            self.assertEqual(1, len({i.date_time for i in entries}), "archive timestamps must be reproducible")

    def test_shipping_image_has_wrapped_launcher_and_bundled_java(self):
        self.assertEqual((ROOT / "client/desktopApp/launcher/porchlight.sh").read_bytes(),
                         (IMAGE / "bin/porchlight").read_bytes())
        for file in ("bin/porchlight", "bin/porchlight-app"):
            self.assertTrue(os.access(IMAGE / file, os.X_OK), "missing executable: " + file)
        self.assertTrue((IMAGE / "lib/runtime/lib/server/libjvm.so").is_file())
        self.assertTrue((IMAGE / "lib/app/porchlight-app.cfg").is_file())


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", type=Path, default=IMAGE)
    args, remaining = parser.parse_known_args()
    IMAGE = args.image
    unittest.main(argv=[__file__, *remaining])
