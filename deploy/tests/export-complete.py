#!/usr/bin/env python3
"""Deterministic archive acceptance fixtures; all staging under var/."""
import gzip
import importlib.util
import io
import json
import pathlib
import sys
import tarfile
import tempfile
import unittest
ROOT = pathlib.Path(__file__).resolve().parents[2]

class Completeness(unittest.TestCase):
    def test_marker_trailer_inventory_and_paths(self):
        spec = importlib.util.spec_from_file_location('verify_export', ROOT / 'deploy/lib/verify-export.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        manifest = {'format_version': 2, 'tables': {'items': 'tables/items.jsonl'}}
        complete = {'format_version': 2, 'complete': True, 'table_rows': {'items': 1}, 'blob_count': 0, 'blob_sha256': []}
        def archive(entries):
            out = io.BytesIO()
            with tarfile.open(fileobj=out, mode='w') as tar:
                for name, value in entries:
                    raw = value if isinstance(value, bytes) else json.dumps(value).encode()
                    info = tarfile.TarInfo(name); info.size = len(raw); tar.addfile(info, io.BytesIO(raw))
            return gzip.compress(out.getvalue())
        entries = [('data.json', manifest), ('tables/items.jsonl', b'{"id":"fixture"}\n'), ('complete.json', complete)]
        with tempfile.TemporaryDirectory(dir=ROOT / 'var') as tmp:
            path = pathlib.Path(tmp) / 'export.gz'
            path.write_bytes(archive(entries)); module.verify(path)
            for data in [archive(entries[:-1]), archive(entries)[:-8], archive(entries + [('extra', b'no')]),
                         archive(entries[:-1] + [('complete.json', dict(complete, table_rows={'items': 2}))]),
                         archive([('../escape', b'no')] + entries), archive(entries[:1] + entries),
                         gzip.compress(gzip.decompress(archive(entries)) + b'nonzero trailing bytes')]:
                path.write_bytes(data)
                with self.assertRaises(Exception): module.verify(path)

if __name__ == '__main__': unittest.main()
