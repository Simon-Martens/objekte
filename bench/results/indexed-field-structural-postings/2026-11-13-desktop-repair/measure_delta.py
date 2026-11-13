#!/usr/bin/env python3
"""Run the temporary desktop delta probe in alternating fresh test processes."""
import argparse
import json
import os
import statistics
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("before", type=Path)
parser.add_argument("after", type=Path)
parser.add_argument("output", type=Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
rows = []
for records in (2048, 8192):
    for loaded in (0, 99, 100):
        for pair in range(6):
            order = ("before", "after") if pair % 2 == 0 else ("after", "before")
            for label in order:
                binary = args.before if label == "before" else args.after
                env = os.environ.copy()
                env["OBJEKTE_DELTA_BENCH_RECORDS"] = str(records)
                env["OBJEKTE_DELTA_BENCH_LOADED"] = str(loaded)
                result = subprocess.run(
                    [str(binary), "--exact", "controller::runtime::tests::desktop_delta_benchmark_probe", "--ignored", "--nocapture"],
                    env=env, text=True, capture_output=True, check=True,
                )
                line = next(line for line in result.stdout.splitlines() if line.startswith("DELTA_BENCH_JSON:"))
                row = json.loads(line.partition(":")[2])
                row.update(revision=label, pair=pair, order=order.index(label))
                rows.append(row)
                print(label, records, loaded, pair, row["elapsed_ns"], row["materialized_hits"], flush=True)
(args.output / "runs.json").write_text(json.dumps(rows, indent=2) + "\n")
summary = []
for records in (2048, 8192):
    for loaded in (0, 99, 100):
        for label in ("before", "after"):
            group = [row for row in rows if row["records"] == records and row["loaded"] == loaded and row["revision"] == label]
            summary.append({
                "records": records,
                "loaded": loaded,
                "revision": label,
                "n": len(group),
                "elapsed_ms_median": statistics.median(row["elapsed_ns"] for row in group) / 1e6,
                "elapsed_ms_min": min(row["elapsed_ns"] for row in group) / 1e6,
                "elapsed_ms_max": max(row["elapsed_ns"] for row in group) / 1e6,
                "materialized_hits": sorted(set(row["materialized_hits"] for row in group)),
                "materialized_bytes": sorted(set(row["materialized_bytes"] for row in group)),
                "returned_hits": sorted(set(row["returned_hits"] for row in group)),
                "residual_source_opens": sorted(set(row["residual_source_opens"] for row in group)),
                "peak_retained_payload_bytes": sorted(set(row["peak_retained_payload_bytes"] for row in group)),
            })
(args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
