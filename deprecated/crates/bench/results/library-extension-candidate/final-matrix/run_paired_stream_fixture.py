#!/usr/bin/env python3
"""Pair the fixed-mtime streaming fixture against unchanged reference core."""
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess

BASE = Path(__file__).resolve().parent
ROOT = BASE.parents[4]
OUT = BASE / "paired-stream-fixture-v2"
BINARIES = {
    "reference": BASE / "candidate-bin/reference-v2/core-allocation",
    "candidate": BASE / "candidate-bin/normal/core-allocation",
}
OUT.mkdir(exist_ok=True)
(OUT / "manifest.json").write_text(json.dumps({
    "harness_source_sha256": hashlib.sha256(
        (ROOT / "crates/bench/src/bin/core_allocation.rs").read_bytes()).hexdigest(),
    "binaries": {label: {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                 for label, path in BINARIES.items()},
    "reference_production_core": "266d9a0fb071 (benchmark-only 2d6d876 harness)",
    "affinity": "0-15", "rayon_threads": 16, "runs_per_case": 5,
}, indent=2, sort_keys=True) + "\n")
summary = {}
for position, case in enumerate(("stream-cache-fixture-off", "stream-cache-fixture-on")):
    pairs = []
    for run in range(1, 6):
        rows = {}
        order = ("reference", "candidate") if (run + position) % 2 else ("candidate", "reference")
        for label in order:
            command = ["taskset", "-c", "0-15", str(BINARIES[label]), "--case", case]
            result = subprocess.run(command, cwd=ROOT, env=dict(os.environ, RAYON_NUM_THREADS="16"),
                                    capture_output=True, text=True)
            stem = f"{case}-run{run}-{label}"
            (OUT / f"{stem}.stdout.json").write_text(result.stdout)
            (OUT / f"{stem}.stderr.log").write_text(result.stderr)
            (OUT / f"{stem}.command.json").write_text(json.dumps({
                "command": command, "exit_status": result.returncode}, indent=2) + "\n")
            if result.returncode:
                raise RuntimeError(f"{stem} exited {result.returncode}")
            rows[label] = json.loads(result.stdout)
            print(stem, round(rows[label]["wall_ms"], 2), flush=True)
        ref, cand = rows["reference"], rows["candidate"]
        if ref["signature"] != cand["signature"]:
            raise RuntimeError(f"{case} run{run}: signature mismatch")
        pairs.append(rows)
    item = {"signature": pairs[0]["reference"]["signature"]}
    for field in ("wall_ms", "allocations", "allocated_bytes", "peak_live_delta_bytes",
                  "rss_high_water_kib"):
        ref = [pair["reference"][field] for pair in pairs]
        cand = [pair["candidate"][field] for pair in pairs]
        item[field] = {"reference": ref, "candidate": cand,
                       "reference_median": statistics.median(ref),
                       "candidate_median": statistics.median(cand),
                       "paired_deltas": [b-a for a,b in zip(ref,cand)]}
    summary[case] = item
    (OUT / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
