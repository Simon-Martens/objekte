#!/usr/bin/env python3
"""Summarize three paired zero-cache JSON query benchmark invocations."""

import argparse
import json
from pathlib import Path
from statistics import median


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--raw", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    source = args.raw.resolve() / "json-schema-ascii-cache0"
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    processes = {
        (pair, revision): [json.loads(line) for line in
                           (source / f"{pair:02d}-{revision}.stdout.ndjson").read_text().splitlines()]
        for pair in range(1, 4)
        for revision in ("reference", "candidate")
    }
    assert all(len(rows) == 153 and rows[0]["query_cache_bytes"] == 0
               for rows in processes.values())
    rows = []
    for index in range(1, 153):
        master = [processes[(pair, "reference")][index] for pair in range(1, 4)]
        current = [processes[(pair, "candidate")][index] for pair in range(1, 4)]
        all_rows = master + current
        signature = {(row["id"], row["filter"], row["matches"], row["first_id"])
                     for row in all_rows}
        assert len(signature) == 1, f"case {index}: result mismatch"
        candidate_counts = {sample["scans"] for row in all_rows for sample in row["samples"]}
        assert len(candidate_counts) == 1, f"case {index}: candidate scan mismatch"
        assert all(sample["cache_hits"] == sample["ordered_hits"] == 0
                   for row in all_rows for sample in row["samples"])
        master_ms = median(row["median_ms"] for row in master)
        current_ms = median(row["median_ms"] for row in current)
        rows.append({
            "id": index, "query": master[0]["filter"],
            "candidate_scans": candidate_counts.pop(),
            "reported_results": master[0]["matches"], "first_id": master[0]["first_id"],
            "master_median_ms": master_ms, "current_median_ms": current_ms,
            "delta_ms": current_ms - master_ms,
            "delta_percent": (current_ms / master_ms - 1) * 100,
            "pair_master_medians_ms": [row["median_ms"] for row in master],
            "pair_current_medians_ms": [row["median_ms"] for row in current],
            "pair_deltas_ms": [c["median_ms"] - m["median_ms"]
                               for m, c in zip(master, current)],
        })
    (output / "summary.json").write_text(json.dumps(rows, indent=2) + "\n")
    table = [
        "| # | Query | Candidate scans | Reported results | Master (ms) | Current (ms) | Δ (ms) | Δ % |",
        "| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for row in rows:
        query = row["query"].replace("|", "\\|")
        table.append(
            f"| {row['id']} | `{query}` | {row['candidate_scans']:,} | "
            f"{row['reported_results']:,} | {row['master_median_ms']:.3f} | "
            f"{row['current_median_ms']:.3f} | {row['delta_ms']:+.3f} | "
            f"{row['delta_percent']:+.1f}% |"
        )
    (output / "QUERY-TABLE.md").write_text("\n".join(table) + "\n")


if __name__ == "__main__":
    main()
