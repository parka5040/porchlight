#!/usr/bin/env python3
"""Run the existing headless JVM smoke tests on the actual shipping desktop jars/runtime.

Called by :shared:packagedDesktopSmoke. Only test classes and test-only dependencies
may augment the image classpath; no system JDK or unfiltered production jar is used.
"""
from pathlib import Path
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[2]
TESTS = [
    "app.porchlight.shared.trust.PlatformTrustTest",
    "app.porchlight.shared.trust.AuditDesktopPackagedBackendTest",
    "app.porchlight.shared.data.DesktopCacheTest",
    "app.porchlight.shared.data.RebaseDesktopCacheTest",
    "app.porchlight.shared.backups.DesktopLaunchTest",
]


def smoke(image, test_only):
    app = image / "lib/app"
    # Check every production class, not just SQLite, to prevent a test classpath from
    # accidentally masking packaging omissions with unfiltered Gradle dependencies.
    production_classes = set()
    for jar in app.glob("*.jar"):
        with zipfile.ZipFile(jar) as archive:
            production_classes.update(n for n in archive.namelist() if n.endswith(".class") and n.rsplit("/", 1)[-1] != "module-info.class")
    isolated = []
    for path in test_only:
        if path.is_dir():
            classes = {p.relative_to(path).as_posix() for p in path.rglob("*.class")}
        elif path.is_file():
            with zipfile.ZipFile(path) as archive:
                classes = set(archive.namelist()) - {"module-info.class"}
        else:
            continue  # Empty Java/resource source sets have no output directory.
        overlap = production_classes.intersection(classes)
        if overlap:
            if path.is_dir():
                raise SystemExit("Smoke classpath contains unfiltered production output: " + sorted(overlap)[0])
            # Tests can resolve a different version of a production dependency. Exclude
            # that entire jar too; production code must always come from the image.
            continue
        isolated.append(path)
    test_only = isolated

    runtime = ROOT / "var"
    runtime.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="desktop-smoke-", dir=runtime) as temporary:
        work = Path(temporary)
        (work / "bin").mkdir()
        (work / "lib/app").mkdir(parents=True)
        for directory in ("trust", "tmp", "home", "skiko"):
            (work / directory).mkdir(mode=0o700)
        # A separate launcher config selects JUnit, without modifying the shipping image.
        launcher = work / "bin/porchlight-app"
        shutil.copy2(image / "bin/porchlight-app", launcher)
        (work / "lib/libapplauncher.so").symlink_to(image / "lib/libapplauncher.so")
        (work / "lib/runtime").symlink_to(image / "lib/runtime")
        config = (app / "porchlight-app.cfg").read_text()
        config = config.replace("$APPDIR", str(app))
        config = config.replace("app.mainclass=app.porchlight.desktop.MainKt", "app.mainclass=org.junit.runner.JUnitCore")
        assert "app.mainclass=org.junit.runner.JUnitCore" in config
        classpath = "".join("app.classpath=" + str(p) + "\n" for p in test_only if p.exists())
        config = config.replace("[JavaOptions]", classpath + "\n[JavaOptions]\n" + "\n".join([
            "java-options=-Djava.awt.headless=true",
            "java-options=-Dporchlight.test.dir=" + str(work / "trust"),
            "java-options=-Djava.io.tmpdir=" + str(work / "tmp"),
            "java-options=-Duser.home=" + str(work / "home"),
            "java-options=-Dskiko.data.path=" + str(work / "skiko"),
        ]))
        (work / "lib/app/porchlight-app.cfg").write_text(config)
        # No user wallet/session, injected JVM options, or system Java on PATH.
        environment = {"PATH": "/nonexistent", "HOME": str(work / "home"), "LANG": "C.UTF-8"}
        result = subprocess.run([str(launcher), *TESTS], env=environment, cwd=ROOT, timeout=180)
        if result.returncode:
            raise SystemExit(result.returncode)
        print("PASS packaged desktop smoke: bundled JVM, packaged SQLite/crypto/cache, no system Java")


if __name__ == "__main__":
    smoke(Path(sys.argv[1]).resolve(), [Path(p).resolve() for p in sys.argv[2:]])
