"""Focused compatibility checks for the declared query benchmark baseline."""

import copy
import json
import tempfile
import unittest
from pathlib import Path

from run_query_regressions import compatibility_failures


class CompatibilityTests(unittest.TestCase):
    def test_semantic_registration_order_and_checkout_path(self):
        baseline = {
            "code_revision": "before", "root": "/first/checkout",
            "corpus_fingerprint": "corpus", "schema_fingerprint": "schema",
            "workload_fingerprint": "workload", "compiler": "rustc",
            "source_cache_bytes": 0, "decoded_cache_bytes": 0, "query_workers": 32,
            "profiles": [["ascii-off", {"alphabet": "ascii", "warm": False,
                                        "verified_cache_bytes": 0}]],
            "registrations": [
                {"name": "a", "indexed_paths": ["x", "y"], "custom_bindings": ["x:n"]},
                {"name": "b", "indexed_paths": ["z"], "custom_bindings": []},
            ],
        }
        queries = [{"id": "Q001", "filter": "x = 'a'"}]
        with tempfile.TemporaryDirectory() as temporary:
            old_dir = Path(temporary) / "before"
            new_dir = Path(temporary) / "after"
            old_dir.mkdir()
            new_dir.mkdir()
            for path in (old_dir, new_dir):
                (path / "queries.json").write_text(json.dumps(queries))
            current = copy.deepcopy(baseline)
            current["code_revision"] = "after"
            current["root"] = "/different/checkout"
            current["registrations"].reverse()
            current["registrations"][0]["indexed_paths"].reverse()
            self.assertEqual(compatibility_failures(old_dir, new_dir, baseline, current), [])
            current["registrations"][0]["indexed_paths"].append("unexpected.field")
            self.assertIn("semantic registrations differ from baseline",
                          compatibility_failures(old_dir, new_dir, baseline, current))
            current = copy.deepcopy(baseline)
            current["code_revision"] = "after"
            queries[0]["filter"] = "title = 'changed'"
            (new_dir / "queries.json").write_text(json.dumps(queries))
            self.assertIn("resolved query workload differs from baseline",
                          compatibility_failures(old_dir, new_dir, baseline, current))


if __name__ == "__main__":
    unittest.main()
