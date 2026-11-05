#!/usr/bin/env python3
"""Repeat the noisy Musenalm counting-allocator rebuild in isolation."""
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess

BASE = Path(__file__).resolve().parent
ROOT = BASE.parents[4]
OUT = BASE / "post-reorg-rebuild-reference-control"
OUT.mkdir(exist_ok=True)
BIN = {
    "reference": BASE / "candidate-bin/reference-v2/core-allocation",
    "candidate": BASE / "candidate-bin/reference-v2/core-allocation",
}
(OUT / "manifest.json").write_text(json.dumps({
    "binaries": {k: {"path": str(v), "sha256": hashlib.sha256(v.read_bytes()).hexdigest()}
                 for k, v in BIN.items()},
    "corpus_sha256": hashlib.sha256((BASE / "corpora-post-reorg.json").read_bytes()).hexdigest(),
    "case": "rebuild", "rayon_threads": 16, "affinity": "0-15", "pairs": 7,
}, indent=2, sort_keys=True) + "\n")
series = []
for run in range(1, 8):
    rows = {}
    for label in (("reference", "candidate") if run % 2 else ("candidate", "reference")):
        cmd = ["taskset", "-c", "0-15", str(BIN[label]), "--case", "rebuild", "--repo",
               "examples/musenalm", "--collection", "contents"]
        result = subprocess.run(cmd, cwd=ROOT, env=dict(os.environ, RAYON_NUM_THREADS="16"),
                                capture_output=True, text=True)
        stem = f"run{run}-{label}"
        (OUT / f"{stem}.stdout.json").write_text(result.stdout)
        (OUT / f"{stem}.stderr.log").write_text(result.stderr)
        (OUT / f"{stem}.command.json").write_text(json.dumps({
            "command": cmd, "exit_status": result.returncode}, indent=2) + "\n")
        if result.returncode:
            raise RuntimeError(f"{stem} exited {result.returncode}")
        rows[label] = json.loads(result.stdout)
        print(stem, round(rows[label]["wall_ms"], 2), flush=True)
    if rows["reference"]["signature"] != rows["candidate"]["signature"]:
        raise RuntimeError(f"rebuild run{run}: signature mismatch")
    series.append(rows)
summary = {}
for metric in ("wall_ms", "allocations", "allocated_bytes", "peak_live_delta_bytes", "rss_high_water_kib"):
    a = [row["reference"][metric] for row in series]
    b = [row["candidate"][metric] for row in series]
    summary[metric] = {"reference": a, "candidate": b,
                       "reference_median": statistics.median(a),
                       "candidate_median": statistics.median(b),
                       "paired_deltas": [y-x for x,y in zip(a,b)],
                       "paired_median_delta": statistics.median(y-x for x,y in zip(a,b))}
(OUT / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
