#!/usr/bin/env python3
"""Repeat two query cases with a reduced workload after the module move."""
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess

BASE = Path(__file__).resolve().parent
ROOT = BASE.parents[4]
OUT = BASE / "post-reorg-targeted"
OUT.mkdir(exist_ok=True)
full = json.loads((ROOT / "crates/bench/workloads/musenalm-mix-100.json").read_text())
selected = []
for source_id in (61, 116):
    row = next(item for item in full if item["id"] == source_id).copy()
    row["id"] = len(selected) + 1
    selected.append(row)
workload = OUT / "workload.json"
workload.write_text(json.dumps(selected, indent=2, ensure_ascii=False) + "\n")
bins = {
    "reference": ROOT / "crates/bench/results/library-extension-baseline/266d9a0fb071-20261104T113205Z/reference-bin/json-query-mix",
    "candidate": BASE / "candidate-bin/post-reorg/json-query-mix",
}
(OUT / "manifest.json").write_text(json.dumps({
    "binaries": {label: {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                 for label, path in bins.items()},
    "workload": str(workload), "workload_sha256": hashlib.sha256(workload.read_bytes()).hexdigest(),
    "source_case_ids": [61, 116], "rayon_threads": 16, "affinity": "0-15", "rounds": 7,
    "pairs": 5,
}, indent=2, sort_keys=True) + "\n")
results = {}
for position, (label, flags) in enumerate((
    ("no-schema-ascii-cache0", ["--no-schema", "--bigram-encoding", "ascii", "--query-cache-mib", "0"]),
    ("schema-utf8-cache64", ["--bigram-encoding", "utf8_bytes", "--query-cache-mib", "64"]),
)):
    samples = {1: [], 2: []}
    for run in range(1, 6):
        paired = {}
        order = ("reference", "candidate") if (run + position) % 2 else ("candidate", "reference")
        for which in order:
            cmd = ["taskset", "-c", "0-15", str(bins[which]), "--repo", "examples/musenalm",
                   "--workload", str(workload), "--runs", "7", *flags]
            result = subprocess.run(cmd, cwd=ROOT, env=dict(os.environ, RAYON_NUM_THREADS="16"),
                                    capture_output=True, text=True)
            stem = f"{label}-pair{run}-{which}"
            (OUT / f"{stem}.stdout.ndjson").write_text(result.stdout)
            (OUT / f"{stem}.stderr.log").write_text(result.stderr)
            (OUT / f"{stem}.command.json").write_text(json.dumps({
                "command": cmd, "exit_status": result.returncode}, indent=2) + "\n")
            if result.returncode:
                raise RuntimeError(f"{stem} exited {result.returncode}")
            rows = [json.loads(line) for line in result.stdout.splitlines() if line]
            if len(rows) != 3:
                raise RuntimeError(f"{stem} returned {len(rows)} rows")
            paired[which] = rows
            print(stem, [round(row["median_ms"], 3) for row in rows[1:]], flush=True)
        for case in (1, 2):
            a, b = paired["reference"][case], paired["candidate"][case]
            timing = {"median_ms", "min_ms", "max_ms", "samples", "source_check_ms"}
            if {k: v for k, v in a.items() if k not in timing} != {
                k: v for k, v in b.items() if k not in timing
            }:
                raise RuntimeError(f"{label} case{case}: signature or work mismatch")
            samples[case].append(b["median_ms"] - a["median_ms"])
    results[label] = {str(case): {"paired_deltas_ms": values,
                                  "paired_median_delta_ms": statistics.median(values)}
                      for case, values in samples.items()}
    (OUT / "summary.json").write_text(json.dumps(results, indent=2, sort_keys=True) + "\n")
