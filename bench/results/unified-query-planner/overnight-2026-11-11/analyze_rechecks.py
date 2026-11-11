#!/usr/bin/env python3
"""Compare paired baseline/final Musenalm CSVs without timing instrumentation."""
import csv
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
NAMES = ("base1", "matched1", "matched2", "base2")
ROWS = {
    name: {
        (row["case_id"], row["profile"]): row
        for row in csv.DictReader((ROOT / f"flagged-{name}" / "query-results.csv").open())
    }
    for name in NAMES
}
keys = set(ROWS["base1"])
assert all(set(rows) == keys for rows in ROWS.values())
work_fields = (
    "total_matches", "returned_records", "candidates", "files_opened",
    "bytes_read", "record_parses", "exact_posting_matches",
    "scanned_records", "residual_evaluations",
)
mismatches = []
repeated = []
for key in sorted(keys):
    a, b, c, d = (ROWS[name][key] for name in NAMES)
    for left, right in ((a, b), (d, c)):
        for field in work_fields:
            if left[field] != right[field]:
                mismatches.append((key, field, left[field], right[field]))
    x, y, z, w = (int(row["median_ns"]) for row in (a, b, c, d))
    if y > x * 1.05 and y - x > 500_000 and z > w * 1.05 and z - w > 500_000:
        repeated.append({
            "case": key[0], "profile": key[1],
            "baseline_round1_ms": x / 1e6, "final_round1_ms": y / 1e6,
            "final_round2_ms": z / 1e6, "baseline_round2_ms": w / 1e6,
            "ratio_round1": y / x, "ratio_round2": z / w,
            "work": {field: a[field] for field in work_fields},
        })
summary = {
    "cases": len({key[0] for key in keys}),
    "profile_rows": len(keys),
    "work_mismatches": mismatches,
    "repeatable_short_run_slowdowns": repeated,
}
(ROOT / "recheck-summary.json").write_text(json.dumps(summary, indent=2) + "\n")
print(json.dumps(summary, indent=2))
