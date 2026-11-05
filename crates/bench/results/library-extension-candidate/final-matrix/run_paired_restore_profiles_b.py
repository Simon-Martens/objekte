#!/usr/bin/env python3
"""Restore identical pinned base-plus-secondary snapshots with both binaries."""
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
PROFILES = [profile for profile in PROFILES if profile[0] in ("leipzig-fts1", "musenalm-fts4")]
BASE = Path(__file__).resolve().parent
OUT = BASE / "paired-restore-profiles-b"
SNAPSHOTS = ROOT / "crates/bench/results/library-extension-baseline/266d9a0fb071-20261104T113205Z/snapshots"
BINARIES = {label: BASE / f"candidate-bin/mode-b/{label}/index-build-profile"
            for label in ("reference", "candidate")}
OUT.mkdir(exist_ok=False)
(OUT / "manifest.json").write_text(json.dumps({
    "binaries": {label: {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                 for label, path in BINARIES.items()},
    "snapshot_directory": str(SNAPSHOTS),
    "baseline_manifest": str(SNAPSHOTS.parent / "manifest.json"),
    "corpora_sha256": hashlib.sha256((BASE / "corpora-before.json").read_bytes()).hexdigest(),
    "features": ["index-timings"],
    "affinity": "0-15", "rayon_threads": 16, "runs": 5,
    "profiles": [profile[0] for profile in PROFILES],
}, indent=2, sort_keys=True) + "\n")
series = {profile[0]: [] for profile in PROFILES}
for run in range(1, 6):
    ordered = PROFILES if run % 2 else list(reversed(PROFILES))
    for position, (name, repo_name, collection, encoding, fields) in enumerate(ordered):
        snapshot = SNAPSHOTS / f"{name}.bigram"
        if not snapshot.is_file() or not snapshot.with_suffix(".secondary").is_file():
            raise FileNotFoundError(snapshot)
        rows = {}
        order = ("reference", "candidate") if (run + position) % 2 else ("candidate", "reference")
        for label in order:
            command = ["taskset", "-c", "0-15", str(BINARIES[label]), "--repo",
                       str(ROOT / "examples" / repo_name), "--collection", collection,
                       "--encoding", encoding, "--snapshot-path", str(snapshot), "--restore-only"]
            for field in fields:
                command.extend(("--field", field))
            result = subprocess.run(command, cwd=ROOT, env=dict(os.environ, RAYON_NUM_THREADS="16"),
                                    capture_output=True, text=True)
            stem = f"{name}-run{run}-{label}"
            (OUT / f"{stem}.stdout.json").write_text(result.stdout)
            (OUT / f"{stem}.stderr.log").write_text(result.stderr)
            (OUT / f"{stem}.command.json").write_text(json.dumps({
                "command": command, "exit_status": result.returncode}, indent=2) + "\n")
            if result.returncode:
                raise RuntimeError(f"{stem} exited {result.returncode}")
            rows[label] = json.loads(result.stdout)
            print(stem, round(rows[label]["snapshot_restore_wall_nanos"] / 1e6, 2), flush=True)
        ref, cand = rows["reference"], rows["candidate"]
        for field in ("documents", "collection", "fields", "encoding", "index_sizes",
                      "compressed_index_bytes", "snapshot_base_bytes", "snapshot_secondary_bytes"):
            if ref[field] != cand[field]:
                raise RuntimeError(f"{name} run{run} {field} mismatch")
        if ref["restore_stages"]["restored_files"] != cand["restore_stages"]["restored_files"]:
            raise RuntimeError(f"{name} run{run} restored file count mismatch")
        series[name].append(rows)
    (OUT / "completed-runs.json").write_text(json.dumps({name: len(rows) for name, rows in series.items()}, indent=2) + "\n")
summary = {}
for name, pairs in series.items():
    item = {field: pairs[0]["reference"][field] for field in
            ("documents", "compressed_index_bytes", "snapshot_base_bytes", "snapshot_secondary_bytes")}
    for field in ("snapshot_restore_wall_nanos", "restore_peak_rss_kib"):
        ref = [pair["reference"][field] for pair in pairs]
        cand = [pair["candidate"][field] for pair in pairs]
        item[field] = {"reference": ref, "candidate": cand,
                       "reference_median": statistics.median(ref),
                       "candidate_median": statistics.median(cand),
                       "paired_deltas": [b-a for a,b in zip(ref,cand)]}
    summary[name] = item
(OUT / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
