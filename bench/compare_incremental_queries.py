#!/usr/bin/env python3
"""Compare direct-master and incremental-view Musenalm query recordings.

The incremental runner records collection definitions at catalogue time. The
direct runner records those definitions plus completed posting/file metadata.
This comparator checks every common definition field and the complete schema
manifest digest, then uses the existing per-query comparison implementation.
"""

import argparse
import json
import subprocess
from pathlib import Path

from compare_query_extensions import compare, load_run, write_results


STATIC_FIELDS = {
    "name",
    "source_path",
    "format",
    "identity",
    "type_hints",
    "sort_fields",
}


def require_compatible(before, after, before_queries, after_queries):
    if before_queries != after_queries:
        raise ValueError("resolved workload queries differ")
    if before.get("workload_fingerprint") != after.get("workload_fingerprint"):
        path = "crates/core/benches/musenalm_queries.json"
        blobs = [
            subprocess.check_output(
                ["git", "rev-parse", f"{run['code_revision']}:{path}"], text=True
            ).strip()
            for run in (before, after)
        ]
        if blobs[0] != blobs[1]:
            raise ValueError("tracked workload manifest changed")
    for key in (
        "compiler",
        "corpus_fingerprint",
        "schema_fingerprint",
        "schema_manifest_digest",
        "query_workers",
        "source_cache_bytes",
        "decoded_cache_bytes",
        "profiles",
        "repetitions",
    ):
        if before.get(key) != after.get(key):
            raise ValueError(f"incompatible {key}: {before.get(key)!r} vs {after.get(key)!r}")
    if after.get("build_mode") != "incremental":
        raise ValueError("after recording is not an incremental view")
    if before.get("build_mode") not in (None, "repository"):
        raise ValueError("before recording is not a direct repository")
    direct = {item["name"]: item for item in before["registrations"]}
    incremental = {item["name"]: item for item in after["registrations"]}
    if len(direct) != len(before["registrations"]) or len(incremental) != len(
        after["registrations"]
    ):
        raise ValueError("duplicate collection registration")
    if direct.keys() != incremental.keys():
        raise ValueError("collection registrations differ")
    for name in direct:
        if set(incremental[name]) != STATIC_FIELDS:
            raise ValueError(f"unexpected incremental registration fields for {name}")
        for key in STATIC_FIELDS:
            left, right = direct[name][key], incremental[name][key]
            if key == "sort_fields":
                left, right = sorted(left), sorted(right)
            if left != right:
                raise ValueError(f"collection {name} differs at {key}")


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
    (args.output / "compatibility.json").write_text(
        json.dumps(
            {
                "before_revision": before["code_revision"],
                "after_revision": after["code_revision"],
                "common_registration_fields": sorted(STATIC_FIELDS),
                "schema_manifest_digest": after["schema_manifest_digest"],
                "corpus_fingerprint": after["corpus_fingerprint"],
                "workload_git_blob": subprocess.check_output(
                    [
                        "git",
                        "rev-parse",
                        f"{after['code_revision']}:crates/core/benches/musenalm_queries.json",
                    ],
                    text=True,
                ).strip(),
            },
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
