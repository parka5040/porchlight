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
# The shipping image is the ProGuard-shrunk release distributable.
IMAGE = ROOT / "client/desktopApp/build/compose/binaries/main-release/app/porchlight"
SIGNATURE = ("META-INF/", (".SF", ".RSA", ".DSA", ".EC"))


def _interface_super_calls(data):
    """(this class, interface, method) for each invokespecial of an interface method that the JVM verifier rejects:
    the referenced interface must be a direct superinterface (JVMS 4.10.1.9) - shrinking can drop one as redundant."""
    import struct
    def u1(o): return data[o]
    def u2(o): return struct.unpack_from(">H", data, o)[0]
    def u4(o): return struct.unpack_from(">I", data, o)[0]
    count, o, pool, i = u2(8), 10, {}, 1
    while i < count:
        tag = u1(o)
        if tag == 1:
            n = u2(o + 1); pool[i] = (1, data[o + 3:o + 3 + n].decode("utf-8", "replace")); o += 3 + n
        elif tag in (7, 8, 16, 19, 20): pool[i] = (tag, u2(o + 1)); o += 3
        elif tag in (3, 4): o += 5
        elif tag in (5, 6): o += 9; i += 1
        elif tag in (9, 10, 11, 12, 17, 18): pool[i] = (tag, u2(o + 1), u2(o + 3)); o += 5
        elif tag == 15: o += 4
        else: raise ValueError("constant pool tag %d" % tag)
        i += 1
    name = lambda k: pool[pool[k][1]][1]
    this = name(u2(o + 2))
    o += 6
    direct = {name(u2(o + 2 + 2 * k)) for k in range(u2(o))}
    o += 2 + 2 * u2(o)
    def attributes(o):  # o at attributes_count; yields (name, offset of attribute) and the offset after them
        found, n, o = [], u2(o), o + 2
        for _ in range(n):
            found.append((pool[u2(o)][1], o)); o += 6 + u4(o + 2)
        return found, o
    fields, o = u2(o), o + 2
    for _ in range(fields):
        o = attributes(o + 6)[1]
    sizes = {**{op: 2 for op in (0x10, 0x12, 0xa9, 0xbc, *range(0x15, 0x1a), *range(0x36, 0x3b))},
             **{op: 3 for op in (0x11, 0x13, 0x14, 0x84, 0xbb, 0xbd, 0xc0, 0xc1, 0xc6, 0xc7, *range(0x99, 0xa9), *range(0xb2, 0xb9))},
             0xc5: 4, 0xb9: 5, 0xba: 5, 0xc8: 5, 0xc9: 5}
    found = []
    methods, o = u2(o), o + 2
    for _ in range(methods):
        code_attributes, o = attributes(o + 6)
        for attribute, a in code_attributes:
            if attribute != "Code":
                continue
            code, end_pc, pc = a + 14, u4(a + 10), 0
            while pc < end_pc:
                op = data[code + pc]
                if op == 0xb7 and pool[u2(code + pc + 1)][0] == 11:
                    ref = pool[u2(code + pc + 1)]
                    owner = name(ref[1])
                    if owner != this and owner not in direct:
                        found.append((this, owner, pool[pool[ref[2]][1]][1]))
                if op in (0xaa, 0xab):
                    base = pc + 1 + (-(pc + 1) % 4)
                    if op == 0xaa:
                        low, high = struct.unpack_from(">ii", data, code + base + 4)
                        pc = base + 12 + 4 * (high - low + 1)
                    else:
                        pc = base + 8 + 8 * u4(code + base + 4)
                elif op == 0xc4:
                    pc += 6 if data[code + pc + 1] == 0x84 else 4
                else:
                    pc += sizes.get(op, 1)
    return found


class AuditDesktopPackagingTest(unittest.TestCase):
    def sqlite(self):
        jars = list((IMAGE / "lib/app").glob("sqlite-jdbc-*.jar"))
        self.assertEqual(1, len(jars), "shipping image must contain exactly one SQLite driver")
        return jars[0]

    def test_only_linux_x64_sqlite_native_is_packaged(self):
        with zipfile.ZipFile(self.sqlite()) as jar:
            natives = [i.filename for i in jar.infolist() if i.filename.startswith(NATIVES) and not i.is_dir()]
        self.assertEqual([LINUX_X64], natives, "shipping SQLite jar includes non-Linux-x64 natives")

    def test_sqlite_keeps_every_class_and_unchanged_resources(self):
        packaged = self.sqlite()
        cache = Path(os.environ.get("GRADLE_USER_HOME", ROOT / "var/gradle-home"))
        sources = [p for p in (cache / "caches/modules-2/files-2.1/org.xerial/sqlite-jdbc").glob("*/*/sqlite-jdbc-*.jar")
                   if packaged.name == p.name or packaged.name.startswith(p.stem + "-")]
        self.assertEqual(1, len(sources), "expected the resolved upstream SQLite jar in the offline cache")
        with zipfile.ZipFile(sources[0]) as source, zipfile.ZipFile(packaged) as target:
            # Shrinking rewrites class files (all org.sqlite classes are kept) and drops multi-release variants
            # (module-info, GraalVM native-image feature); every other entry must be byte-identical.
            expected = {i.filename: source.read(i) for i in source.infolist()
                        if not i.is_dir() and not i.filename.startswith("META-INF/versions/")
                        and (not i.filename.startswith(NATIVES) or i.filename == LINUX_X64)}
            actual = {i.filename: target.read(i) for i in target.infolist() if not i.is_dir()}
            self.assertEqual(set(expected), set(actual), "only foreign natives and multi-release variants may be removed")
            for name, content in expected.items():
                if not name.endswith(".class"):
                    self.assertEqual(content, actual[name], "changed SQLite entry: " + name)
            self.assertIn(b"org.sqlite.JDBC", actual["META-INF/services/java.sql.Driver"])
            entries = target.infolist()
            self.assertEqual(len(entries), len({i.filename for i in entries}), "duplicate zip entries")
            self.assertEqual(1, len({i.date_time for i in entries}), "archive timestamps must be reproducible")

    def test_shrunk_jars_carry_no_stale_signatures(self):
        # ProGuard rewrites signed jars (BouncyCastle); a kept upstream signature would make the JVM reject them.
        for jar in (IMAGE / "lib/app").glob("*.jar"):
            with zipfile.ZipFile(jar) as archive:
                signed = [n for n in archive.namelist() if n.startswith(SIGNATURE[0]) and n.count("/") == 1
                          and n.upper().endswith(SIGNATURE[1])]
            self.assertEqual([], signed, "stale signature in shrunk jar: " + jar.name)

    def test_shrunk_classes_keep_every_direct_superinterface_they_call(self):
        hazards = []
        for jar in sorted((IMAGE / "lib/app").glob("*.jar")):
            with zipfile.ZipFile(jar) as archive:
                for entry in archive.namelist():
                    if entry.endswith(".class") and not entry.startswith("META-INF/"):
                        hazards += _interface_super_calls(archive.read(entry))
        self.assertEqual([], hazards[:10], "%d invokespecial targets are not direct superinterfaces (VerifyError)" % len(hazards))

    def test_every_service_descriptor_names_a_shipped_class(self):
        # Shrinking can remove a provider that a META-INF/services file still names (ServiceConfigurationError).
        archives = [zipfile.ZipFile(jar) for jar in sorted((IMAGE / "lib/app").glob("*.jar"))]
        classes = {n[:-6].replace("/", ".") for a in archives for n in a.namelist() if n.endswith(".class")}
        stale = [(Path(a.filename).name, n, line.split("#")[0].strip()) for a in archives for n in a.namelist()
                 if n.startswith("META-INF/services/") and not n.endswith("/")
                 for line in a.read(n).decode("utf-8").splitlines()
                 if line.split("#")[0].strip() and line.split("#")[0].strip() not in classes]
        self.assertEqual([], stale, "service descriptors name classes missing from the shipping image")

    def test_client_smoke_never_launches_the_users_real_profile(self):
        # The packaged app opens $XDG_DATA_HOME/porchlight: a CI launch with the user's environment would act as the
        # user's enrolled device against their real server.
        smoke = (ROOT / "scripts/smoke-clients.sh").read_text()
        launch = next(line for line in smoke.splitlines() if '"$launcher"' in line and "&" in line)
        start = smoke.index(launch)
        env = smoke.rfind("\nenv ", 0, start)
        self.assertGreaterEqual(env, 0, "the packaged desktop launch must run under env with a private profile")
        command = smoke[env:start + len(launch)]
        for variable in ("HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"):
            self.assertIn(variable + '="$profile', command, variable + " must point into the private smoke profile")
        self.assertIn('profile=$(mktemp -d "$root/var/', smoke)

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
