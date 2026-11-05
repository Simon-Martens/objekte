#!/usr/bin/env python3
"""Interleave all core-allocation cases on one deterministic fixture."""
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[5]
sys.path.insert(0, str(ROOT / "crates/bench"))
from run_core_allocation import SYNTHETIC, MUSENALM, STREAM, clear_synthetic_cache  # noqa: E402
BASE = Path(__file__).resolve().parent
OUT = BASE / "post-reorg-core-allocation"
FIXTURE = Path(json.loads((BASE / "allocation-fixture.json").read_text())["path"])
BINARIES = {
    "reference": ROOT / "crates/bench/results/library-extension-candidate/final-matrix/candidate-bin/reference-v2/core-allocation",
    "candidate": BASE / "candidate-bin/post-reorg/core-allocation",
}
OUT.mkdir(exist_ok=False)
(OUT / "manifest.json").write_text(json.dumps({
    "binaries": {label: {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                 for label, path in BINARIES.items()},
    "fixture": json.loads((BASE / "allocation-fixture.json").read_text()),
    "musenalm": json.loads((BASE / "corpora-post-reorg.json").read_text())["musenalm"],
    "affinity": "0-15", "rayon_threads": 16, "runs_per_case": 5,
}, indent=2, sort_keys=True) + "\n")
summary = {}
for corpus, cases, root in (("synthetic-20k", SYNTHETIC, FIXTURE),
                            ("musenalm-contents", MUSENALM, ROOT / "examples/musenalm"),
                            ("stream-cache-fixture-2k", STREAM, FIXTURE)):
    for position, case in enumerate(cases):
        pairs = []
        for run in range(1, 6):
            rows = {}
            order = ("reference", "candidate") if (run + position) % 2 else ("candidate", "reference")
            for label in order:
                if corpus == "synthetic-20k" and case == "cold-build":
                    clear_synthetic_cache(root)
                command = ["taskset", "-c", "0-15", str(BINARIES[label]),
                           "--case", case, "--repo", str(root)]
                if corpus == "musenalm-contents":
                    command += ["--collection", "contents", "--filter", "year >= 1700",
                                "--sort", "annotation ASC", "--pattern", "und"]
                result = subprocess.run(command, cwd=ROOT, env=dict(os.environ, RAYON_NUM_THREADS="16"),
                                        capture_output=True, text=True)
                stem = f"{corpus}-{case}-run{run}-{label}"
                (OUT / f"{stem}.stdout.json").write_text(result.stdout)
                (OUT / f"{stem}.stderr.log").write_text(result.stderr)
                (OUT / f"{stem}.command.json").write_text(json.dumps({
                    "command": command, "exit_status": result.returncode}, indent=2) + "\n")
                if result.returncode:
                    raise RuntimeError(f"{stem} exited {result.returncode}")
                rows[label] = json.loads(result.stdout)
                print(stem, round(rows[label]["wall_ms"], 2), flush=True)
            ref, cand = rows["reference"], rows["candidate"]
            if ref["signature"] != cand["signature"] or ref["case"] != cand["case"]:
                raise RuntimeError(f"{corpus} {case} run{run}: signature mismatch")
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
        summary[f"{corpus}/{case}"] = item
        (OUT / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
