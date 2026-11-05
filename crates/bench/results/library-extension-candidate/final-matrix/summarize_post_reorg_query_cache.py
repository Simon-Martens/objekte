#!/usr/bin/env python3
"""Check every full cache-run row and retain per-case cross-build deltas."""
import json
from pathlib import Path
import statistics

BASE = Path(__file__).resolve().parent
reference = [json.loads(line) for line in (BASE / "post-reorg-query-cache-reference.ndjson").open()]
candidate = [json.loads(line) for line in (BASE / "post-reorg-query-cache-candidate.ndjson").open()]
if len(reference) != len(candidate):
    raise RuntimeError(f"row count differs: {len(reference)} != {len(candidate)}")

TIMING = {"ms", "median_ms", "min_ms", "max_ms", "source_check_ms",
          "allocations", "allocated_bytes", "peak_live_delta_bytes"}

def stable(row):
    value = {key: entry for key, entry in row.items()
             if key not in TIMING and not key.endswith("_ms")
             and not key.startswith(("rss_", "peak_rss_"))}
    if "samples" in value:
        value["samples"] = [{key: entry for key, entry in sample.items()
                             if key not in TIMING} for sample in value["samples"]]
    return value

groups = {}
for position, (a, b) in enumerate(zip(reference, candidate)):
    if stable(a) != stable(b):
        raise RuntimeError(f"semantic or source-work mismatch at row {position}")
    if a["type"] == "setup":
        continue
    identity = tuple(a.get(key) for key in (
        "experiment", "comparison_budget_mib", "cache_mib", "type", "id",
        "scenario", "label", "filter", "offset", "repetition", "pattern"))
    groups.setdefault(identity, []).append((a.get("ms", a.get("median_ms")),
                                             b.get("ms", b.get("median_ms"))))

summary = []
for identity, pairs in groups.items():
    if len(pairs) != 7:
        raise RuntimeError(f"{identity}: expected 7 samples, got {len(pairs)}")
    a = [pair[0] for pair in pairs]
    b = [pair[1] for pair in pairs]
    deltas = [y-x for x,y in pairs]
    summary.append({"identity": identity, "reference_median_ms": statistics.median(a),
                    "candidate_median_ms": statistics.median(b),
                    "paired_deltas_ms": deltas,
                    "paired_median_delta_ms": statistics.median(deltas)})
output = {"rows_per_build": len(reference), "semantics_and_work_equal": True,
          "groups": sorted(summary, key=lambda row: str(row["identity"]))}
(BASE / "post-reorg-query-cache-cross-build-summary.json").write_text(
    json.dumps(output, indent=2, sort_keys=True) + "\n")
print(len(reference), "rows and", len(summary), "groups match")
