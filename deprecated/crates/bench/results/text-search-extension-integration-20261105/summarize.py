#!/usr/bin/env python3
"""Recompute the focused text-definition comparison from committed raw rows."""

import hashlib
import json
from pathlib import Path
from statistics import median


ROOT = Path(__file__).resolve().parent


def read(name):
    return json.loads((ROOT / name).read_text())


def samples(rows):
    values = [row["ms"] for row in rows]
    return {"median_ms": median(values), "min_ms": min(values), "max_ms": max(values)}


def main():
    reference = read("reference-frontend.json")
    candidate = read("candidate-frontend.json")
    identical = all(
        reference[key] == candidate[key]
        for key in ("total", "truncated", "ordered_ids", "indexed_sources")
    )
    frontend = {}
    for route in ("scan", "indexed"):
        before = [row for row in reference["samples"] if row["path"] == route]
        after = [row for row in candidate["samples"] if row["path"] == route]
        earlier = samples(before)
        later = samples(after)
        frontend[route] = {
            "reference": earlier,
            "candidate": later,
            "delta_ms": later["median_ms"] - earlier["median_ms"],
            "relative_pct": 100 * (later["median_ms"] / earlier["median_ms"] - 1),
            "reference_work": sorted({
                (row["source_reads"], row["parses"], row["candidates"])
                for row in before
            }),
            "candidate_work": sorted({
                (row["source_reads"], row["parses"], row["candidates"])
                for row in after
            }),
        }
    for metric in ("index_build_ms", "snapshot_restore_ms", "snapshot_bytes"):
        before = reference[metric]
        after = candidate[metric]
        frontend[metric] = {"reference": before, "candidate": after,
                            "delta": after - before,
                            "relative_pct": 100 * (after / before - 1)}

    focused = {}
    signatures = {}
    for budget in (0, 64):
        rows = [json.loads(line) for line in
                (ROOT / f"focused-cache{budget}.ndjson").read_text().splitlines()]
        if len(rows) != 104:
            raise ValueError(f"cache {budget}: expected 1 setup + 12 signatures + 91 samples")
        focused[str(budget)] = {"setup": rows[0], "cases": {}}
        signatures[budget] = {
            row["query"]: {"total": row["total"], "ids": row["ids"]}
            for row in rows if row["case"] == "signature"
        }
        for name in sorted({row["case"] for row in rows if "round" in row}):
            selected = [row for row in rows if row.get("case") == name and "round" in row]
            if len(selected) != 7:
                raise ValueError(f"cache {budget}, {name}: expected seven rounds")
            focused[str(budget)]["cases"][name] = {
                **samples(selected),
                "total": selected[0]["total"],
                "returned": len(selected[0]["ids"]),
                "source_reads": sorted({row["source_reads"] for row in selected}),
                "parses": sorted({row["parses"] for row in selected}),
                "candidate_visits": sorted({row["candidates"] for row in selected}),
                "fts_candidates": sorted({row["fts_candidates"] for row in selected}),
                "ordered_cache_hits": sorted({row["ordered_result_hits"] for row in selected}),
            }
    if signatures[0] != signatures[64]:
        raise ValueError("0/64 MiB complete signatures differ")
    signature = hashlib.sha256(json.dumps(signatures[0], sort_keys=True).encode()).hexdigest()
    result = {"frontend_complete_results_equal": identical,
              "frontend": frontend,
              "focused_complete_signatures_equal": True,
              "focused_signature_sha256": signature,
              "focused": focused}
    (ROOT / "summary.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"frontend_results_equal": identical,
                      "focused_signatures_equal": True,
                      "focused_signature_sha256": signature}))


if __name__ == "__main__":
    main()
