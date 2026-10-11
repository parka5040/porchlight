#!/usr/bin/env python3
"""Check the cross-APK ABI after building debug and both minified APKs (no emulator)."""
from pathlib import Path
import re
import struct
import unittest
from zipfile import ZipFile

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "client/androidApp/build/outputs"


class Dex:
    def __init__(self, data):
        def u32(offset):
            return struct.unpack_from("<I", data, offset)[0]

        def uleb(offset):
            value, shift = 0, 0
            while True:
                byte = data[offset]
                offset += 1
                value |= (byte & 127) << shift
                if byte < 128:
                    return value, offset
                shift += 7

        def type_list(offset):
            if not offset:
                return []
            return [types[struct.unpack_from("<H", data, offset + 4 + i * 2)[0]]
                    for i in range(u32(offset))]

        strings = []
        for i in range(u32(56)):
            _, start = uleb(u32(u32(60) + i * 4))
            strings.append(data[start:data.index(b"\0", start)].decode("utf-8", errors="replace"))
        types = [strings[u32(u32(68) + i * 4)] for i in range(u32(64))]
        protos = []
        for i in range(u32(72)):
            offset = u32(76) + i * 12
            protos.append("(" + "".join(type_list(u32(offset + 8))) + ")" + types[u32(offset + 4)])
        self.fields, self.methods = [], []
        for i in range(u32(80)):
            owner, kind, name = struct.unpack_from("<HHI", data, u32(84) + i * 8)
            self.fields.append((types[owner], strings[name], types[kind]))
        for i in range(u32(88)):
            owner, proto, name = struct.unpack_from("<HHI", data, u32(92) + i * 8)
            self.methods.append((types[owner], strings[name], protos[proto]))
        self.classes = {}
        for i in range(u32(96)):
            offset = u32(100) + i * 32
            name, parent = types[u32(offset)], u32(offset + 8)
            parents = ([] if parent == 0xffffffff else [types[parent]]) + type_list(u32(offset + 12))
            members = set()
            pos = u32(offset + 24)
            if pos:
                counts = []
                for _ in range(4):
                    count, pos = uleb(pos)
                    counts.append(count)
                for group, count in enumerate(counts):
                    index = 0
                    for _ in range(count):
                        delta, pos = uleb(pos)
                        index += delta
                        _, pos = uleb(pos)  # access flags
                        if group >= 2:
                            _, pos = uleb(pos)  # method code offset
                        members.add((self.fields if group < 2 else self.methods)[index][1:])
            self.classes[name] = (parents, members)


def apk(variant):
    path, = (OUTPUT / "apk" / variant).glob("*.apk")
    classes, references = {}, set()
    with ZipFile(path) as archive:
        for name in archive.namelist():
            if name.endswith(".dex"):
                dex = Dex(archive.read(name))
                classes.update(dex.classes)
                references.update(dex.fields)
                references.update(dex.methods)
    return classes, references


def resolves(classes, owner, member, visited=None):
    visited = set() if visited is None else visited
    if owner not in classes or owner in visited:
        return False
    visited.add(owner)
    parents, members = classes[owner]
    if member[0] == "<init>":
        return member in members
    return member in members or any(resolves(classes, parent, member, visited) for parent in parents)


class AuditApkFix1Test(unittest.TestCase):
    def test_runner_tracing_entrypoints_survive_in_target(self):
        target, _ = apk("minified")
        self.assertTrue("Landroidx/tracing/Trace;" in target,
                        "fix1: AndroidJUnitRunner cannot start without target androidx.tracing.Trace")
        for method, signature in (("beginSection", "(Ljava/lang/String;)V"),
                                  ("endSection", "()V"), ("forceEnableAppTracing", "()V")):
            self.assertTrue(resolves(target, "Landroidx/tracing/Trace;", (method, signature)))

    def test_instrumentation_links_target_runtime(self):
        original, _ = apk("debug")
        target, _ = apk("minified")
        tests, references = apk("androidTest/minified")
        combined = target | tests
        # Test R8 applies the target mapping, including parameter/return types. Restore those
        # descriptors only when deciding whether an original app member is a linkage candidate.
        unmapped = {}
        for line in (OUTPUT / "mapping/minified/mapping.txt").read_text().splitlines():
            match = re.fullmatch(r"(\S+) -> (\S+):", line)
            if match:
                before, after = ("L" + name.replace(".", "/") + ";" for name in match.groups())
                unmapped[after] = before
        missing = []
        for owner, name, signature in sorted(references):
            if owner not in original:
                continue
            original_signature = re.sub(r"L[^;]+;", lambda m: unmapped.get(m[0], m[0]), signature)
            if owner not in combined or (resolves(original, owner, (name, original_signature))
                                        and not resolves(combined, owner, (name, signature))):
                missing.append(f"{owner} {name}{signature}")
        for parents, _ in tests.values():
            missing.extend(parent for parent in parents if parent in original and parent not in combined)
        self.assertEqual(missing, [], "fix1: instrumentation references app ABI removed by R8")

    def test_testing_rules_do_not_enter_release(self):
        config = (OUTPUT / "mapping/release/configuration.txt").read_text()
        self.assertFalse("proguard-minified-test.pro" in config, "Test rules leaked into release")
        minified = (OUTPUT / "mapping/minified/configuration.txt").read_text()
        self.assertTrue("proguard-minified-test.pro" in minified, "Missing minified-only test rules")


if __name__ == "__main__":
    unittest.main()
