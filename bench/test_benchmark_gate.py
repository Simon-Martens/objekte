"""Focused compatibility checks for durable benchmark schema evidence."""

import copy
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from run_query_regressions import compatibility_failures
from schema_evidence import compare_schema_identity, schema_identity


def write_schemas(directory: Path, schemas: list[dict]) -> str:
    canonical = json.dumps(schemas, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    digest = hashlib.sha256(canonical.encode()).hexdigest()
    (directory / "schema-manifest.json").write_text(json.dumps({
        "version": 1, "schemas": schemas, "canonical_json": canonical, "sha256": digest,
    }))
    return digest


def entry(name: str, **schema: object) -> dict:
    return {"collection": name, "path": f".objekte/collections/{name}.schema.json",
            "schema": {"x-objekte": {"name": name}, **schema}}


class CompatibilityTests(unittest.TestCase):
    def manifests(self):
        baseline = {
            "code_revision": "before", "root": "/first/checkout",
            "corpus_fingerprint": "corpus", "schema_fingerprint": "path-hash",
            "workload_fingerprint": "workload", "compiler": "rustc",
            "source_cache_bytes": 0, "decoded_cache_bytes": 0, "query_workers": 32,
            "profiles": [["ascii-off", {"alphabet": "ascii", "warm": False,
                                       "verified_cache_bytes": 0}]],
            "registrations": [
                {"name": "a", "indexed_paths": ["x", "y"], "custom_bindings": ["x:n"]},
                {"name": "b", "indexed_paths": ["z"], "custom_bindings": []},
            ],
        }
        current = copy.deepcopy(baseline)
        current["code_revision"] = "after"
        current["root"] = "/different/checkout"
        return baseline, current

    def test_missing_evidence_never_certifies_matching_schemas(self):
        with tempfile.TemporaryDirectory() as temporary:
            old_dir, new_dir = (Path(temporary) / name for name in ("before", "after"))
            old_dir.mkdir()
            new_dir.mkdir()
            for path in (old_dir, new_dir):
                (path / "queries.json").write_text("[]")
            old, new = self.manifests()
            self.assertTrue(any("unverifiable schema baseline" in failure
                                for failure in compatibility_failures(old_dir, new_dir, old, new)))
            write_schemas(old_dir, [entry("a"), entry("b")])
            self.assertTrue(any("unverifiable schema baseline" in failure
                                for failure in compatibility_failures(old_dir, new_dir, old, new)))

    def test_saved_evidence_survives_checkout_changes_and_checks_semantics(self):
        with tempfile.TemporaryDirectory() as temporary:
            old_dir, new_dir = (Path(temporary) / name for name in ("before", "after"))
            old_dir.mkdir()
            new_dir.mkdir()
            for path in (old_dir, new_dir):
                (path / "queries.json").write_text('[{"id":"Q001","filter":"x = 1"}]')
            old, new = self.manifests()
            old["root"] = str(Path(temporary) / "source-before")
            new["root"] = str(Path(temporary) / "source-after")
            for root in (old["root"], new["root"]):
                source = Path(root) / ".objekte/collections"
                source.mkdir(parents=True)
                (source / "a.schema.json").write_text('{"before":true}')
            docs = [entry("a", properties={"x": {"minimum": 1, "type": "number"}}), entry("b")]
            old["schema_manifest_digest"] = write_schemas(old_dir, docs)
            new["schema_manifest_digest"] = write_schemas(new_dir, copy.deepcopy(docs))
            new["schema_fingerprint"] = "different-checkout-path-hash"
            new["registrations"].reverse()
            new["registrations"][0]["indexed_paths"].reverse()
            self.assertEqual(compatibility_failures(old_dir, new_dir, old, new), [])
            for root in (old["root"], new["root"]):
                (Path(root) / ".objekte/collections/a.schema.json").write_text('{"after":true}')
            self.assertEqual(compatibility_failures(old_dir, new_dir, old, new), [])
            for root in (old["root"], new["root"]):
                (Path(root) / ".objekte/collections/a.schema.json").unlink()
            self.assertEqual(compatibility_failures(old_dir, new_dir, old, new), [])
            changed = copy.deepcopy(docs)
            changed[0]["schema"]["properties"]["x"]["minimum"] = 2
            new["schema_manifest_digest"] = write_schemas(new_dir, changed)
            self.assertIn("semantic schema differs from baseline",
                          compatibility_failures(old_dir, new_dir, old, new))
            new["schema_manifest_digest"] = write_schemas(new_dir, docs)
            new["registrations"][0]["indexed_paths"].append("unexpected")
            self.assertIn("semantic registrations differ from baseline",
                          compatibility_failures(old_dir, new_dir, old, new))
            new["registrations"][0]["indexed_paths"].remove("unexpected")
            (new_dir / "queries.json").write_text('[{"id":"Q001","filter":"x = 2"}]')
            self.assertIn("resolved query workload differs from baseline",
                          compatibility_failures(old_dir, new_dir, old, new))

    def test_schema_artifact_validation_matrix(self):
        with tempfile.TemporaryDirectory() as temporary:
            old_dir, new_dir = (Path(temporary) / name for name in ("before", "after"))
            old_dir.mkdir()
            new_dir.mkdir()
            self.assertEqual(schema_identity(old_dir, set())[0], None)
            self.assertEqual(schema_identity(new_dir, set())[0], None)
            write_schemas(old_dir, [])
            write_schemas(new_dir, [])
            self.assertEqual(compare_schema_identity(old_dir, new_dir, set(), set(), None, None), [])
            docs = [entry("a", required=["x", "y"]), entry("b")]
            old_digest = write_schemas(old_dir, docs)
            reordered = copy.deepcopy(docs)
            reordered[0]["schema"] = {"required": ["x", "y"], "x-objekte": {"name": "a"}}
            new_digest = write_schemas(new_dir, reordered)
            self.assertEqual(old_digest, new_digest)
            self.assertEqual(compare_schema_identity(old_dir, new_dir, {"a", "b"}, {"a", "b"}, old_digest, new_digest), [])
            self.assertIsNotNone(schema_identity(new_dir, {"a", "b", "c"})[1])
            reordered[0]["schema"]["required"] = ["y", "x"]
            write_schemas(new_dir, reordered)
            self.assertIn("semantic schema differs from baseline",
                          compare_schema_identity(old_dir, new_dir, {"a", "b"}, {"a", "b"}, None, None))
            (new_dir / "schema-manifest.json").write_text("not json")
            self.assertIsNotNone(schema_identity(new_dir, {"a", "b"})[1])

    def test_numeric_precision_and_declared_digest_are_retained(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            canonical = ('[{"collection":"a","path":".objekte/collections/a.schema.json",'
                         '"schema":{"minimum":0.1000000000000000000000000000000001,'
                         '"x-objekte":{"name":"a"}}}]')
            digest = hashlib.sha256(canonical.encode()).hexdigest()
            artifact = ('{"version":1,"canonical_json":' + json.dumps(canonical) +
                        ',"schemas":' + canonical + ',"sha256":' + json.dumps(digest) + '}')
            (directory / "schema-manifest.json").write_text(artifact)
            self.assertEqual(schema_identity(directory, {"a"})[0], digest)
            self.assertIsNotNone(schema_identity(directory, {"a"}, "0" * 64)[1])
            (directory / "schema-manifest.json").write_text(artifact.replace(
                "0000000000000000000000000001", "0000000000000000000000000002", 1))
            self.assertIsNotNone(schema_identity(directory, {"a"})[1])


if __name__ == "__main__":
    unittest.main()
