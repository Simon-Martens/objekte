#!/usr/bin/env python3
"""Isolate timer and counting-allocator effects on the Leipzig FTS1 build."""
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[5]
BASE = Path(__file__).resolve().parent
OUT = BASE / "leipzig-harness-control"
OUT.mkdir(exist_ok=False)
manifest = {"profile": "leipzig-fts1", "repo": "examples/leipzig-multilingual",
            "collection": "texts", "encoding": "utf8_bytes", "fields": ["text=plain"],
            "affinity": "0-15", "rayon_threads": 16, "warmups": 1, "pairs": 5,
            "modes": {}}
for mode in ("D",):
    binaries = {"reference": ROOT / "crates/bench/results/library-extension-baseline/266d9a0fb071-20261104T113205Z/reference-bin-index/index-build-profile",
                "candidate": BASE / "candidate-bin/mode-d/reference/index-build-profile"}
    manifest["modes"][mode] = {label: {"path": str(path),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest()} for label, path in binaries.items()}
(OUT / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
summary = {}
for mode in ("D",):
    binaries = {"reference": ROOT / "crates/bench/results/library-extension-baseline/266d9a0fb071-20261104T113205Z/reference-bin-index/index-build-profile",
                "candidate": BASE / "candidate-bin/mode-d/reference/index-build-profile"}
    pairs = []
    for run in range(0, 6):
        rows = {}
        order = ("reference", "candidate") if run % 2 else ("candidate", "reference")
        for label in order:
            command = ["taskset", "-c", "0-15", str(binaries[label]), "--repo",
                       str(ROOT / "examples/leipzig-multilingual"), "--collection", "texts",
                       "--encoding", "utf8_bytes", "--field", "text=plain", "--skip-snapshot"]
            result = subprocess.run(command, cwd=ROOT, env=dict(os.environ, RAYON_NUM_THREADS="16"),
                                    capture_output=True, text=True)
            name = f"mode{mode}-run{run}-{label}"
            (OUT / f"{name}.stdout.json").write_text(result.stdout)
            (OUT / f"{name}.stderr.log").write_text(result.stderr)
            (OUT / f"{name}.command.json").write_text(json.dumps({"command": command,
                "exit_status": result.returncode}, indent=2) + "\n")
            if result.returncode:
                raise RuntimeError(f"{name} exited {result.returncode}")
            row = json.loads(result.stdout)
            if row.get("build_allocations") is None or row.get("stages") is None:
                raise RuntimeError(f"{name} instrumentation differs")
            rows[label] = row
            print(name, round(row["build_wall_nanos"] / 1e6, 2), flush=True)
        ref, cand = rows["reference"], rows["candidate"]
        for field in ("documents", "collection", "fields", "encoding", "index_sizes", "compressed_index_bytes"):
            if ref[field] != cand[field]:
                raise RuntimeError(f"mode{mode} run{run} {field} mismatch")
        if run:
            pairs.append(rows)
    result = {"documents": pairs[0]["reference"]["documents"],
              "compressed_index_bytes": pairs[0]["reference"]["compressed_index_bytes"]}
    for field in ("build_wall_nanos", "build_allocations", "build_allocated_bytes",
                  "build_peak_live_delta_bytes", "build_peak_rss_kib"):
        if pairs[0]["reference"].get(field) is None:
            continue
        ref = [pair["reference"][field] for pair in pairs]
        cand = [pair["candidate"][field] for pair in pairs]
        result[field] = {"reference": ref, "candidate": cand,
                         "reference_median": statistics.median(ref),
                         "candidate_median": statistics.median(cand),
                         "paired_deltas": [b-a for a,b in zip(ref,cand)]}
    if mode == "D":
        for field in ("general_values", "source_read", "json_parse", "field_text"):
            ref = [pair["reference"]["stages"][field]["cpu_nanos"] for pair in pairs]
            cand = [pair["candidate"]["stages"][field]["cpu_nanos"] for pair in pairs]
            result[f"{field}_cpu_nanos"] = {"reference": ref, "candidate": cand,
                "paired_deltas": [b-a for a,b in zip(ref,cand)]}
    summary[mode] = result
(OUT / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
