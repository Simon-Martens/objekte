#!/usr/bin/env python3
"""Pair encoding builds and save/restore on the same real and synthetic inputs."""
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[5]
OUT = Path(__file__).resolve().parent / "paired-bigram"
REF = ROOT / "crates/bench/results/library-extension-baseline/266d9a0fb071-20261104T113205Z/reference-bin/bigram-encoding-compare"
CAND = OUT.parent / "candidate-bin/normal/bigram-encoding-compare"
OUT.mkdir(exist_ok=False)
(OUT / "manifest.json").write_text(json.dumps({
    "reference_sha256": hashlib.sha256(REF.read_bytes()).hexdigest(),
    "candidate_sha256": hashlib.sha256(CAND.read_bytes()).hexdigest(),
    "corpus": json.loads((OUT.parent / "corpora-before.json").read_text())["musenalm"],
    "rayon_threads": 16, "affinity": "0-15", "pairs": 5,
}, indent=2, sort_keys=True) + "\n")
series = {}
for run in range(1, 6):
    for source in ("examples/musenalm/data/contents", "synthetic:24000"):
        for encoding in ("ascii", "utf8_bytes"):
            key = (source, encoding)
            results = {}
            order = ("reference", "candidate") if (run + (source.startswith("synthetic")) + (encoding == "utf8_bytes")) % 2 else ("candidate", "reference")
            for label in order:
                binary = REF if label == "reference" else CAND
                command = ["taskset", "-c", "0-15", str(binary), encoding, source]
                completed = subprocess.run(command, cwd=ROOT, env=dict(os.environ, RAYON_NUM_THREADS="16"),
                                           capture_output=True, text=True)
                name = f"{'musenalm' if source.startswith('examples/') else 'synthetic'}-{encoding}-{run}-{label}"
                (OUT / f"{name}.stdout.json").write_text(completed.stdout)
                (OUT / f"{name}.stderr.log").write_text(completed.stderr)
                (OUT / f"{name}.command.json").write_text(json.dumps({
                    "command": command, "exit_status": completed.returncode}, indent=2) + "\n")
                if completed.returncode:
                    raise RuntimeError(f"{name} exited {completed.returncode}")
                results[label] = json.loads(completed.stdout)
                print(name, round(results[label]["build_ms"], 3), flush=True)
            ref, cand = results["reference"], results["candidate"]
            for field in ("dataset", "encoding", "files", "compressed_bytes", "snapshot_bytes"):
                if ref[field] != cand[field]:
                    raise RuntimeError(f"{name} {field} mismatch: {ref[field]} != {cand[field]}")
            if [(row["term"], row["candidates"]) for row in ref["queries"]] != [
                (row["term"], row["candidates"]) for row in cand["queries"]]:
                raise RuntimeError(f"{name} candidate mismatch")
            series.setdefault(key, []).append(results)
summary = {}
for (source, encoding), pairs in series.items():
    name = f"{'musenalm' if source.startswith('examples/') else 'synthetic'}-{encoding}"
    item = {"files": pairs[0]["reference"]["files"],
            "compressed_bytes": pairs[0]["reference"]["compressed_bytes"],
            "snapshot_bytes": pairs[0]["reference"]["snapshot_bytes"]}
    for field in ("build_ms", "save_ms", "restore_ms", "build_peak_rss_kib"):
        ref = [pair["reference"][field] for pair in pairs]
        cand = [pair["candidate"][field] for pair in pairs]
        item[field] = {"reference": ref, "candidate": cand,
                       "reference_median": statistics.median(ref),
                       "candidate_median": statistics.median(cand),
                       "paired_deltas": [b-a for a,b in zip(ref,cand)]}
    summary[name] = item
(OUT / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
