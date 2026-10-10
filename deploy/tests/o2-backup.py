#!/usr/bin/env python3
"""O2a lease/projection regressions, no external tools or key material."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('backup', Path(__file__).parents[1] / 'backup/backup.py')
backup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(backup)

class Tests(unittest.TestCase):
    def test_lease_commits_renews_and_releases_on_exception(self):
        with patch.object(backup, 'sql', side_effect=['00000000-0000-0000-0000-000000000001','t','']) as sql:
            with self.assertRaises(RuntimeError):
                with backup.BlobLease('backup:test') as lease:
                    lease.renew(force=True)
                    raise RuntimeError('copy failed')
            self.assertIn('pl_take_blob_lease', sql.call_args_list[0].args[0])
            self.assertIn('pl_renew_blob_lease', sql.call_args_list[1].args[0])
            self.assertIn('DELETE FROM porchlight.blob_leases', sql.call_args_list[2].args[0])
    def test_release_failure_logs_fixed_text_without_database_details(self):
        import io
        stderr = io.StringIO()
        with patch.object(backup, 'sql', side_effect=['00000000-0000-0000-0000-000000000001', RuntimeError('secret database diagnostic')]), patch('sys.stderr', stderr):
            with backup.BlobLease('backup:test'): pass
        self.assertIn('backup blob lease release failed', stderr.getvalue())
        self.assertNotIn('secret', stderr.getvalue())
    def test_expired_lease_cannot_publish(self):
        with patch.object(backup, 'sql', side_effect=['00000000-0000-0000-0000-000000000001','f','']):
            with backup.BlobLease('backup:test') as lease:
                with self.assertRaises(ValueError): lease.renew(force=True)
    def test_digest_rechecks_lease_after_slow_hash_before_publication(self):
        class Lease:
            def renew(self, force=False):
                self.forced = force
                raise ValueError('expired during encrypted digest')
        lease = Lease()
        with patch.object(backup.ops, 'digest', return_value={'size': 100, 'sha256': 'a'*64}) as digest:
            with self.assertRaises(ValueError): backup.publication_digest(Path('fixture'), lease)
            digest.assert_called_once_with(Path('fixture'))
            self.assertTrue(lease.forced)
    def test_final_projection_uses_both_hashes_and_lease(self):
        class Snapshot:
            def __init__(self): self.queries=[]
            def query(self, query):
                self.queries.append(query)
                if 'to_regclass' in query: return 't'
                if 'to_jsonb(b)' in query: return '[{"household_id":"h","size":4,"sha256":"'+'a'*64+'"},{"household_id":"h","size":8,"sha256":"'+'b'*64+'"}]'
                return '[{"household_id":"h","blob_sha256":"'+'a'*64+'","thumbnail_sha256":"'+'b'*64+'"}]'
        s=Snapshot()
        self.assertEqual(backup.referenced_blobs(s,'00000000-0000-0000-0000-000000000001'), {
            'aa/aa/'+'a'*64: {'sha256': 'a'*64, 'size': 4},
            'bb/bb/'+'b'*64: {'sha256': 'b'*64, 'size': 8}})
        self.assertTrue(any('blob_lease_items' in q and 'attachments' in q for q in s.queries))

if __name__ == '__main__': unittest.main()
