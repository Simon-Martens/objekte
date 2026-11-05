#!/usr/bin/env python3
"""Compare the two requested seven-round JSON query invocations."""

import csv
import json
from pathlib import Path
from statistics import median


ROOT = Path(__file__).resolve().parent
BUDGETS = (64, 0)
FIELDS = ("matches", "first_id", "scans", "source_reads", "parses", "cache_hits", "ordered_hits")


def load(phase, budget):
    path = ROOT / phase / f"json-schema-ascii-cache{budget}.ndjson"
    rows = [json.loads(line) for line in path.read_text().splitlines()]
    setup = [row for row in rows if row.get("type") == "setup"]
    cases = [row for row in rows if row.get("type") == "case"]
    assert len(setup) == 1 and setup[0]["cases"] == 152 and setup[0]["runs"] == 7
    assert len(cases) == 152 and [row["id"] for row in cases] == list(range(1, 153))
    assert all(len(row["samples"]) == 7 for row in cases)
    return setup[0], cases


def main():
    all_rows = []
    summaries = {}
    for budget in BUDGETS:
        ref_setup, reference = load("reference", budget)
        cand_setup, candidate = load("candidate", budget)
        assert ref_setup["query_cache_bytes"] == cand_setup["query_cache_bytes"]
        for setup in (ref_setup, cand_setup):
            assert setup["bigram_encoding"] == "ascii"
            assert setup["schema_mode"] == "configured"
            assert (setup["query_cache_bytes"] == 0) == (budget == 0)
        differences = []
        for ref, cand in zip(reference, candidate):
            assert (ref["id"], ref["filter"], ref["kind"]) == (
                cand["id"], cand["filter"], cand["kind"])
            if any(ref[field] != cand[field] for field in FIELDS):
                differences.append(ref["id"])
            delta = cand["median_ms"] - ref["median_ms"]
            all_rows.append({
                "cache_mib": budget, "id": ref["id"], "kind": ref["kind"],
                "filter": ref["filter"], "reference_ms": ref["median_ms"],
                "candidate_ms": cand["median_ms"], "delta_ms": delta,
                "delta_pct": 100 * delta / ref["median_ms"] if ref["median_ms"] else 0,
                "reference_min_ms": ref["min_ms"], "reference_max_ms": ref["max_ms"],
                "candidate_min_ms": cand["min_ms"], "candidate_max_ms": cand["max_ms"],
                **{f"reference_{field}": ref[field] for field in FIELDS},
                **{f"candidate_{field}": cand[field] for field in FIELDS},
            })
        rows = [row for row in all_rows if row["cache_mib"] == budget]
        summaries[str(budget)] = {
            "cases": len(rows), "signature_and_work_differences": differences,
            "sum_reference_case_medians_ms": sum(row["reference_ms"] for row in rows),
            "sum_candidate_case_medians_ms": sum(row["candidate_ms"] for row in rows),
            "median_case_delta_ms": median(row["delta_ms"] for row in rows),
            "largest_positive_deltas": sorted(rows, key=lambda row: row["delta_ms"], reverse=True)[:12],
            "plausible_material_slowdowns": [row for row in rows
                if row["delta_ms"] > 10 and row["delta_pct"] > 10],
        }
    with (ROOT / "per-query.csv").open("w", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=list(all_rows[0]), lineterminator="\n")
        writer.writeheader()
        writer.writerows(all_rows)
    (ROOT / "comparison.json").write_text(json.dumps(summaries, indent=2) + "\n")
    for budget, summary in summaries.items():
        print(budget, "MiB", "sum", round(summary["sum_reference_case_medians_ms"], 2),
              "->", round(summary["sum_candidate_case_medians_ms"], 2),
              "work differences", summary["signature_and_work_differences"],
              "suspicious", [row["id"] for row in summary["plausible_material_slowdowns"]])


if __name__ == "__main__":
    main()
