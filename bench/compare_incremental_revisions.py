#!/usr/bin/env python3
"""Compare two complete incremental-view Musenalm query recordings."""

import argparse
import json
import subprocess
from pathlib import Path

from compare_query_extensions import compare, load_run, write_results


def require_compatible(before, after, before_queries, after_queries):
    if before.get("build_mode") != "incremental" or after.get("build_mode") != "incremental":
        raise ValueError("both recordings must use incremental views")
    if before_queries != after_queries:
        raise ValueError("resolved workload queries differ")
    for key in (
        "compiler", "corpus_fingerprint", "schema_fingerprint",
        "schema_manifest_digest", "query_workers", "source_cache_bytes",
        "decoded_cache_bytes", "profiles", "repetitions",
    ):
        if before.get(key) != after.get(key):
            raise ValueError(f"incompatible {key}")
    if before["registrations"] != after["registrations"]:
        raise ValueError("static collection registrations differ")
    if before.get("workload_fingerprint") != after.get("workload_fingerprint"):
        path = "crates/core/benches/musenalm_queries_extended.json"
        blobs = [
            subprocess.check_output(
                ["git", "rev-parse", f"{run['code_revision']}:{path}"], text=True
            ).strip()
            for run in (before, after)
        ]
        if blobs[0] != blobs[1]:
            raise ValueError("tracked extended workload changed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    before, before_queries, before_rows = load_run(args.before)
    after, after_queries, after_rows = load_run(args.after)
    require_compatible(before, after, before_queries, after_queries)
    rows = compare(before_rows, after_rows, after_queries)
    write_results(args.output, rows, before, after)
    before_digests = json.loads((args.before / "result-digests.json").read_text())
    after_digests = json.loads((args.after / "result-digests.json").read_text())
    if before_digests != after_digests:
        raise ValueError("normalized result digests changed")
    if any(int(row["after_files_opened"]) > int(row["before_files_opened"]) for row in rows):
        raise ValueError("source-open regression")
    (args.output / "compatibility.json").write_text(json.dumps({
        "before_revision": before["code_revision"],
        "after_revision": after["code_revision"],
        "compared_rows": len(rows),
        "result_digests_equal": True,
        "source_open_increases": 0,
        "schema_manifest_digest": after["schema_manifest_digest"],
        "corpus_fingerprint": after["corpus_fingerprint"],
    }, indent=2) + "\n")


if __name__ == "__main__":
    main()
