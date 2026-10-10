#!/usr/bin/env python3
"""Offline checks for the committed table, including a newer Python Unicode DB."""
import contextlib
import io
from pathlib import Path
import runpy
import sys
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("generate-filename-categories.py")
TABLE = SCRIPT.parent.parent / "server/api/src/blobs/filename_categories.rs"


class TableChecks(unittest.TestCase):
    def test_other_unicode_version_verifies_then_skips_regeneration(self):
        output = io.StringIO()
        with patch.object(sys, "argv", [str(SCRIPT), "--check"]), patch(
            "unicodedata.unidata_version", "99.0.0"
        ), patch("unicodedata.category", side_effect=AssertionError("must not regenerate")), contextlib.redirect_stdout(output):
            runpy.run_path(str(SCRIPT), run_name="__main__")
        self.assertIn("SKIP (python unicodedata 99.0.0 ≠ recorded 16.0.0)", output.getvalue())

    def test_corrupt_table_rejected_even_with_other_unicode_version(self):
        content = TABLE.read_text().replace("0x7f..=0x9f", "0x7e..=0x9f")
        module = runpy.run_path(str(SCRIPT))
        with self.assertRaises(AssertionError):
            module["verify_committed"](content)

    def test_corrupt_recorded_version_rejected(self):
        content = TABLE.read_text().replace("Unicode 16.0.0", "Unicode 99.0.0")
        module = runpy.run_path(str(SCRIPT))
        with self.assertRaises(AssertionError):
            module["verify_committed"](content)


if __name__ == "__main__":
    unittest.main()
