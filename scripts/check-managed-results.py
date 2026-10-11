#!/usr/bin/env python3
"""Fail closed on missing, empty or failed UTP JUnit results, even if Gradle succeeded."""
from pathlib import Path
import sys
import xml.etree.ElementTree as ET


def check_device(directory):
    reports = sorted(directory.rglob("TEST-*.xml"))
    if not reports:
        raise ValueError("missing result XML")
    exit_codes = sorted(directory.rglob("test-result-exit-code.txt"))
    if not exit_codes:
        raise ValueError("missing UTP completion code")
    for code in exit_codes:
        if code.read_text().strip() != "0":
            raise ValueError("UTP run failed or crashed")
    total = 0
    for report in reports:
        root = ET.parse(report).getroot()
        if root.tag not in ("testsuite", "testsuites"):
            raise ValueError("invalid JUnit root")
        suites = [node for node in root.iter() if node.tag in ("testsuite", "testsuites")]
        for suite in suites:
            tests = int(suite.attrib["tests"])
            if tests <= 0:
                raise ValueError("zero tests (runner crash or empty selection)")
            for count in ("errors", "failures"):
                if int(suite.get(count, "0")) != 0:
                    raise ValueError("failed or crashed tests")
            if not 0 <= int(suite.get("skipped", "0")) < tests:
                raise ValueError("no executed tests")
            if len(list(suite.iter("testcase"))) != tests:
                raise ValueError("incomplete test results")
        if any(node.tag in ("error", "failure") for node in root.iter()):
            raise ValueError("failed or crashed tests")
        cases = list(root.iter("testcase"))
        executed = sum(case.find("skipped") is None for case in cases)
        if not executed:
            raise ValueError("no executed tests")
        total += executed
    return total


def main():
    if len(sys.argv) < 3:
        print("usage: check-managed-results.py RESULTS_DIR DEVICE [DEVICE ...]", file=sys.stderr)
        return 2
    results = Path(sys.argv[1])
    failed = False
    for device in sys.argv[2:]:
        try:
            count = check_device(results / device)
        except (OSError, ET.ParseError, ValueError, KeyError):
            # Reports/logs can contain enrollment or other private values; never echo their content.
            # Only diagnostic categories created here are safe to print.
            failed = True
            print(f"managed results: {device}: FAIL (missing, zero tests, incomplete, failed or crashed run)",
                  file=sys.stderr)
        else:
            print(f"managed results: {device}: PASS ({count} executed tests)")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
