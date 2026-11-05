#!/usr/bin/env python3
"""Interleave preserved NYC query-cache executables under fixed affinity."""
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[5]
OUT = Path(__file__).resolve().parent / "nyc-paired"
REF = ROOT / "crates/bench/results/library-extension-baseline/266d9a0fb071-20261104T113205Z/reference-bin/query-cache"
CAND = OUT.parent / "candidate-bin/normal/query-cache"
OUT.mkdir(exist_ok=False)
manifest = {"reference": {"path": str(REF), "sha256": hashlib.sha256(REF.read_bytes()).hexdigest()},
            "candidate": {"path": str(CAND), "sha256": hashlib.sha256(CAND.read_bytes()).hexdigest()},
            "corpus": json.loads((OUT.parent / "corpora-before.json").read_text())["nyc311"],
            "rayon_threads": 16, "affinity": "0-15", "pairs": 7, "budgets_mib": [0, 64]}
(OUT / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
summary = {}
for budget in (0, 64):
    pairs = []
    for pair in range(7):
        rows = {}
        order = ("reference", "candidate") if (pair + budget // 64) % 2 == 0 else ("candidate", "reference")
        for label in order:
            binary = REF if label == "reference" else CAND
            command = ["taskset", "-c", "0-15", str(binary), "--repo", "examples/nyc311",
                       "--collection", "requests", "--text-pattern", "Park", "--encoding", "ascii",
                       "--cache-mib", str(budget), "--run", str(pair)]
            result = subprocess.run(command, cwd=ROOT, env=dict(os.environ, RAYON_NUM_THREADS="16"),
                                    capture_output=True, text=True)
            name = f"budget{budget}-pair{pair}-{label}"
            (OUT / f"{name}.stdout.ndjson").write_text(result.stdout)
            (OUT / f"{name}.stderr.log").write_text(result.stderr)
            (OUT / f"{name}.command.json").write_text(json.dumps({"command": command,
                "exit_status": result.returncode}, indent=2) + "\n")
            if result.returncode:
                raise RuntimeError(f"{name} exited {result.returncode}")
            parsed = [json.loads(line) for line in result.stdout.splitlines()]
            steps = [row for row in parsed if row.get("type") == "text_step"]
            if len(steps) != 2:
                raise RuntimeError(f"{name} emitted {len(steps)} text steps")
            rows[label] = steps
            print(name, [round(row["ms"], 3) for row in steps], flush=True)
        for ref, cand in zip(rows["reference"], rows["candidate"]):
            for field in ("ids", "total", "truncated", "scans", "parses", "predicate_cache_hits",
                          "ordered_result_hits", "cache_bytes", "repetition"):
                if ref[field] != cand[field]:
                    raise RuntimeError(f"budget{budget} pair{pair} {field} mismatch")
        pairs.append(rows)
    summary[str(budget)] = {}
    for repetition in (0, 1):
        ref = [pair["reference"][repetition]["ms"] for pair in pairs]
        cand = [pair["candidate"][repetition]["ms"] for pair in pairs]
        delta = [b - a for a, b in zip(ref, cand)]
        summary[str(budget)][str(repetition)] = {"reference_ms": ref, "candidate_ms": cand,
            "paired_deltas_ms": delta, "reference_median_ms": statistics.median(ref),
            "candidate_median_ms": statistics.median(cand),
            "paired_median_delta_ms": statistics.median(delta)}
(OUT / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
