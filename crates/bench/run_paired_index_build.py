#!/usr/bin/env python3
"""Compare two index-build-profile binaries with identical CPU affinity."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import time


FIELDS = (
    "annotation=markdown",
    "preferred_title=plain",
    "title_statement=html",
    "contributors[].annotation=xml",
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=7)
    parser.add_argument("--cpus", default="0-15")
    parser.add_argument("--threads", type=int, default=16)
    args = parser.parse_args()
    if args.runs < 1 or args.threads < 1:
        parser.error("runs and threads must be positive")
    root = Path(__file__).resolve().parents[2]
    repo = root / "examples/musenalm"
    if not (repo / "data/contents").is_dir():
        parser.error("materialize examples/musenalm/data/contents first")
    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=False)
    binaries = {"reference": args.reference.resolve(), "candidate": args.candidate.resolve()}
    for label, binary in binaries.items():
        if not binary.is_file():
            parser.error(f"missing {label} binary: {binary}")
    revision = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=root, text=True
    ).strip()
    manifest = {
        "revision": revision,
        "reference_revision": "266d9a0fb071838be8f757671599872a7036a054",
        "binaries": {label: {"path": str(path), "sha256": sha256(path)}
                     for label, path in binaries.items()},
        "repo": str(repo),
        "cpus": args.cpus,
        "rayon_num_threads": args.threads,
        "runs_per_binary": args.runs,
        "fields": list(FIELDS),
        "affinity_before_taskset": sorted(os.sched_getaffinity(0)),
        "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    (output_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    env = os.environ.copy()
    env["RAYON_NUM_THREADS"] = str(args.threads)
    results = {"reference": [], "candidate": []}
    for round_number in range(1, args.runs + 1):
        order = ("reference", "candidate") if round_number % 2 else (
            "candidate", "reference")
        for label in order:
            command = [
                "taskset", "-c", args.cpus, str(binaries[label]),
                "--repo", str(repo), "--collection", "contents",
                "--encoding", "ascii",
            ]
            for field in FIELDS:
                command.extend(("--field", field))
            started = time.monotonic()
            result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True)
            name = f"{round_number:02d}-{label}"
            (output_dir / f"{name}.stdout.json").write_text(result.stdout)
            (output_dir / f"{name}.stderr.log").write_text(result.stderr)
            (output_dir / f"{name}.command.json").write_text(json.dumps({
                "command": command,
                "rayon_num_threads": args.threads,
                "exit_status": result.returncode,
                "elapsed_wall_seconds": time.monotonic() - started,
            }, indent=2) + "\n")
            if result.returncode:
                raise RuntimeError(f"{name} failed: {result.stderr}")
            record = json.loads(result.stdout)
            if record["stages"]["workers"] != args.threads:
                raise RuntimeError(f"{name} used {record['stages']['workers']} workers")
            if record["documents"] != 133948 or record["fields"] != list(FIELDS):
                raise RuntimeError(f"{name} has unexpected benchmark coverage")
            results[label].append(record)
            print(f"{name}: {record['build_wall_nanos']/1e6:.1f} ms", flush=True)
    summary = {}
    for label, records in results.items():
        values = [record["build_wall_nanos"] / 1e6 for record in records]
        summary[label] = {
            "build_wall_ms": values,
            "median_ms": statistics.median(values),
            "min_ms": min(values),
            "max_ms": max(values),
            "workers": sorted({record["stages"]["workers"] for record in records}),
            "documents": sorted({record["documents"] for record in records}),
            "compressed_index_bytes": sorted({record["compressed_index_bytes"]
                                              for record in records}),
        }
    ref = summary["reference"]["median_ms"]
    cand = summary["candidate"]["median_ms"]
    summary["candidate_minus_reference_ms"] = cand - ref
    summary["candidate_minus_reference_percent"] = (cand / ref - 1) * 100
    (output_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(f"median: reference {ref:.1f} ms, candidate {cand:.1f} ms "
          f"({(cand/ref-1)*100:+.1f}%)", flush=True)


if __name__ == "__main__":
    main()
