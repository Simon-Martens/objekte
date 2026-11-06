#!/usr/bin/env python3
"""Compare complete, protocol-matched Musenalm query recordings."""

import argparse
import csv
import json
from collections import defaultdict
from pathlib import Path


def load_run(directory: Path):
    manifest = json.loads((directory / "run-manifest.json").read_text())
    queries = json.loads((directory / "queries.json").read_text())
    with (directory / "query-results.csv").open(newline="") as source:
        rows = list(csv.DictReader(source))
    by_key = {(row["case_id"], row["profile"]): row for row in rows}
    if len(by_key) != len(rows):
        raise ValueError(f"duplicate case/profile in {directory}")
    return manifest, queries, by_key


def require_compatible(before, after, before_queries, after_queries):
    if before_queries != after_queries:
        raise ValueError("resolved workload queries differ")
    for key in (
        "compiler",
        "corpus_fingerprint",
        "schema_fingerprint",
        "schema_manifest_digest",
        "workload_fingerprint",
        "query_workers",
        "source_cache_bytes",
        "decoded_cache_bytes",
        "profiles",
        "repetitions",
    ):
        if before.get(key) != after.get(key):
            raise ValueError(f"incompatible {key}: {before.get(key)!r} vs {after.get(key)!r}")


def compare(before_rows, after_rows, queries):
    if before_rows.keys() != after_rows.keys():
        raise ValueError("case/profile coverage differs")
    by_id = {query["id"]: query for query in queries}
    output = []
    for case_id, profile in sorted(before_rows):
        before = before_rows[(case_id, profile)]
        after = after_rows[(case_id, profile)]
        if before["status"] != "ok" or after["status"] != "ok":
            raise ValueError(f"failed sample: {case_id}/{profile}")
        if before["samples"] != after["samples"]:
            raise ValueError(f"sample count changed: {case_id}/{profile}")
        for field in ("total_matches", "returned_records"):
            if before[field] != after[field]:
                raise ValueError(f"answer changed: {case_id}/{profile}/{field}")
        before_ms = int(before["median_ns"]) / 1_000_000
        after_ms = int(after["median_ns"]) / 1_000_000
        delta_ms = after_ms - before_ms
        output.append({
            "case_id": case_id,
            "profile": profile,
            "query_json": json.dumps(by_id[case_id], ensure_ascii=False, sort_keys=True),
            "before_ms": f"{before_ms:.6f}",
            "after_ms": f"{after_ms:.6f}",
            "delta_ms": f"{delta_ms:+.6f}",
            "delta_percent": f"{delta_ms / before_ms * 100:+.2f}" if before_ms else "n/a",
            "before_candidates": before["candidates"],
            "after_candidates": after["candidates"],
            "before_files_opened": before["files_opened"],
            "after_files_opened": after["files_opened"],
            "before_scanned_records": before["scanned_records"],
            "after_scanned_records": after["scanned_records"],
            "total_matches": after["total_matches"],
            "returned_records": after["returned_records"],
        })
    return output


def write_results(output_dir: Path, rows, before, after):
    output_dir.mkdir(parents=True, exist_ok=True)
    with (output_dir / "comparison.csv").open("w", newline="") as destination:
        writer = csv.DictWriter(destination, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)
    profiles = defaultdict(list)
    for row in rows:
        profiles[row["profile"]].append(row)
    lines = [
        "# Query extension comparison",
        "",
        f"Before code: `{before['code_revision']}`. After code: `{after['code_revision']}`.",
        "All resolved queries, corpus/schema fingerprints, compiler, worker and cache settings, profiles, repetitions, counts and page sizes match.",
        "Delta is after minus before. Each time is the median of the timed samples in milliseconds.",
        "Warm-cache percentage changes at microsecond scale are descriptive; the table separately counts changes of at least 1 ms.",
        "",
        "| Profile | Before sum of medians, ms | After sum of medians, ms | Delta, ms | Queries >= 1 ms slower | Queries > 5% slower |",
        "| --- | ---: | ---: | ---: | ---: | ---: |",
    ]
    for profile, group in sorted(profiles.items()):
        old = sum(float(row["before_ms"]) for row in group)
        new = sum(float(row["after_ms"]) for row in group)
        slower_ms = sum(float(row["delta_ms"]) >= 1 for row in group)
        slower_pct = sum(
            row["delta_percent"] != "n/a" and float(row["delta_percent"]) > 5
            for row in group
        )
        lines.append(
            f"| {profile} | {old:.3f} | {new:.3f} | {new-old:+.3f} | {slower_ms} | {slower_pct} |"
        )
    lines.extend([
        "",
        "## Complete per-query table",
        "",
        "The full query expression and options are in the `query_json` column; source and scan work is shown beside every timing.",
        "",
        "| Case | Profile | Query JSON | Before ms | After ms | Delta ms | Delta % | Candidates before/after | Opens before/after | Scanned before/after | Matches | Page |",
        "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ])
    for row in rows:
        query = row["query_json"].replace("|", "\\|")
        lines.append(
            f"| {row['case_id']} | {row['profile']} | {query} | {row['before_ms']} | {row['after_ms']} | {row['delta_ms']} | {row['delta_percent']} | "
            f"{row['before_candidates']}/{row['after_candidates']} | {row['before_files_opened']}/{row['after_files_opened']} | "
            f"{row['before_scanned_records']}/{row['after_scanned_records']} | {row['total_matches']} | {row['returned_records']} |"
        )
    (output_dir / "COMPARISON.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines[:len(profiles) + 10]))
    print(f"Saved {len(rows)} complete comparison rows in {output_dir}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("output", type=Path)
    arguments = parser.parse_args()
    before, before_queries, before_rows = load_run(arguments.before)
    after, after_queries, after_rows = load_run(arguments.after)
    require_compatible(before, after, before_queries, after_queries)
    rows = compare(before_rows, after_rows, after_queries)
    write_results(arguments.output, rows, before, after)


if __name__ == "__main__":
    main()
