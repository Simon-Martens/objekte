#!/usr/bin/env python3
"""Compare staged-construction query results with the predecessor baseline.

Correctness mismatches fail the command. Timing flags are evidence to investigate,
not an automatic verdict, because concurrent construction and cache warmth matter.
"""

from __future__ import annotations

import argparse
import csv
import json
from pathlib import Path


def read_json(path: Path):
    return json.loads(path.read_text())


def result_rows(directory: Path) -> dict[tuple[str, str], dict[str, str]]:
    with (directory / "query-results.csv").open(newline="") as source:
        rows = list(csv.DictReader(source))
    keyed = {(row["case_id"], row["profile"]): row for row in rows}
    if len(keyed) != len(rows):
        raise ValueError(f"duplicate case/profile rows in {directory}")
    return keyed


def compare(baseline: Path, current: Path) -> dict:
    old_manifest = read_json(baseline / "run-manifest.json")
    new_manifest = read_json(current / "run-manifest.json")
    old_queries = {query["id"]: query for query in read_json(baseline / "queries.json")}
    new_queries = {query["id"]: query for query in read_json(current / "queries.json")}
    old_digests = read_json(baseline / "result-digests.json")
    new_digests = read_json(current / "result-digests.json")
    old_rows = result_rows(baseline)
    new_rows = result_rows(current)
    mismatches = []
    for key in (
        "corpus_fingerprint", "compiler", "schema_manifest_digest",
        "query_workers", "source_cache_bytes", "decoded_cache_bytes",
        "prepared_cache_bytes",
    ):
        if old_manifest.get(key) != new_manifest.get(key):
            mismatches.append(f"{key} differs")
    if old_manifest.get("profiles") != new_manifest.get("profiles"):
        mismatches.append("profiles or verified-cache settings differ")
    if old_manifest.get("failures") != 0 or new_manifest.get("failures") != 0:
        mismatches.append("a benchmark run reports failures")
    if new_manifest.get("repetitions", 0) < 3:
        mismatches.append("current run has fewer than three repetitions")
    for case_id, query in old_queries.items():
        if new_queries.get(case_id) != query:
            mismatches.append(f"{case_id}: query definition changed or is missing")
        if new_digests.get(case_id) != old_digests.get(case_id):
            mismatches.append(f"{case_id}: result ID/evidence/order digest differs")
    flags = []
    ratios = {}
    for key, old in old_rows.items():
        new = new_rows.get(key)
        if new is None:
            mismatches.append(f"{key}: case/profile missing")
            continue
        for field in ("status", "total_matches", "returned_records"):
            if old[field] != new[field]:
                mismatches.append(f"{key}: {field} {old[field]} -> {new[field]}")
        old_ns = int(old["median_ns"])
        new_ns = int(new["median_ns"])
        ratio = new_ns / max(old_ns, 1)
        ratios[f"{key[0]}/{key[1]}"] = ratio
        if ratio > 1.05 and new_ns - old_ns > 500_000:
            flags.append({
                "case": key[0], "profile": key[1], "old_ns": old_ns,
                "new_ns": new_ns, "ratio": round(ratio, 4),
                "delta_ns": new_ns - old_ns,
            })
    flags.sort(key=lambda row: row["delta_ns"], reverse=True)
    return {
        "baseline_revision": old_manifest["code_revision"],
        "current_revision": new_manifest["code_revision"],
        "compared_cases": len(old_queries),
        "new_cases": sorted(set(new_queries) - set(old_queries)),
        "compared_profiles": len(old_rows),
        "correctness_mismatches": mismatches,
        "regression_flags": flags,
        "median_latency_ratio_by_case_profile": ratios,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("current", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    report = compare(args.baseline, args.current)
    rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(rendered)
    else:
        print(rendered)
    return bool(report["correctness_mismatches"])


if __name__ == "__main__":
    raise SystemExit(main())
