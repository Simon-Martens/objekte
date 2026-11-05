#!/usr/bin/env python3
"""Interleave reference and candidate index-build-profile for all declared profiles."""
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[5]
sys.path.insert(0, str(ROOT / "crates/bench"))
from run_index_build_profiles import PROFILES  # noqa: E402
OUT = Path(__file__).resolve().parent / "paired-index-profiles-normal"
REF = ROOT / "crates/bench/results/library-extension-candidate/final-matrix/candidate-bin/reference-mode-a/index-build-profile"
CAND = OUT.parent / "candidate-bin/mode-a/index-build-profile"
OUT.mkdir(exist_ok=False)
(OUT / "manifest.json").write_text(json.dumps({
    "reference_sha256": hashlib.sha256(REF.read_bytes()).hexdigest(),
    "candidate_sha256": hashlib.sha256(CAND.read_bytes()).hexdigest(),
    "corpora_sha256": hashlib.sha256((OUT.parent / "corpora-before.json").read_bytes()).hexdigest(),
    "features": [], "rayon_threads": 16,
    "affinity": "0-15", "runs": 5,
    "profiles": [profile[0] for profile in PROFILES],
}, indent=2, sort_keys=True) + "\n")
series = {profile[0]: [] for profile in PROFILES}
for run in range(1, 6):
    ordered = PROFILES if run % 2 else list(reversed(PROFILES))
    for position, (name, repo_name, collection, encoding, fields) in enumerate(ordered):
        records = {}
        labels = ("reference", "candidate") if (run + position) % 2 else ("candidate", "reference")
        for label in labels:
            binary = REF if label == "reference" else CAND
            command = ["taskset", "-c", "0-15", str(binary), "--repo", str(ROOT / "examples" / repo_name),
                       "--collection", collection, "--encoding", encoding]
            for field in fields:
                command.extend(("--field", field))
            command.append("--skip-snapshot")
            result = subprocess.run(command, cwd=ROOT, env=dict(os.environ, RAYON_NUM_THREADS="16"),
                                    capture_output=True, text=True)
            stem = f"{name}-run{run}-{label}"
            (OUT / f"{stem}.stdout.json").write_text(result.stdout)
            (OUT / f"{stem}.stderr.log").write_text(result.stderr)
            (OUT / f"{stem}.command.json").write_text(json.dumps({
                "command": command, "exit_status": result.returncode,
            }, indent=2) + "\n")
            if result.returncode:
                raise RuntimeError(f"{stem} exited {result.returncode}")
            record = json.loads(result.stdout)
            if record.get("allocation_counting") is not False or record.get("stage_timers") is not False:
                raise RuntimeError(f"{stem}: wrong instrumentation")
            records[label] = record
            print(stem, round(record["build_wall_nanos"] / 1e6, 2), flush=True)
        ref, cand = records["reference"], records["candidate"]
        for key in ("documents", "collection", "encoding", "fields", "compressed_index_bytes",
                    "index_sizes"):
            if ref[key] != cand[key]:
                raise RuntimeError(f"{name} run{run}: {key} mismatch")
        series[name].append(records)
    (OUT / "completed-runs.json").write_text(json.dumps({name: len(rows) for name, rows in series.items()}, indent=2) + "\n")
summary = {}
for name, pairs in series.items():
    result = {"documents": pairs[0]["reference"]["documents"],
              "compressed_index_bytes": pairs[0]["reference"]["compressed_index_bytes"],
              "index_sizes": pairs[0]["reference"]["index_sizes"]}
    for metric in ("build_wall_nanos", "build_peak_rss_kib", "build_ready_rss_kib"):
        ref = [pair["reference"][metric] for pair in pairs]
        cand = [pair["candidate"][metric] for pair in pairs]
        result[metric] = {"reference": ref, "candidate": cand,
                          "reference_median": statistics.median(ref),
                          "candidate_median": statistics.median(cand),
                          "paired_deltas": [b - a for a, b in zip(ref, cand)]}
    summary[name] = result
(OUT / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
