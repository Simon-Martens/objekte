#!/usr/bin/env python3
"""Run the benchmark-only index I/O variants in alternating order."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[2]
PROFILES_PATH = ROOT / "crates/bench/run_index_build_profiles.py"
spec = importlib.util.spec_from_file_location("index_profiles", PROFILES_PATH)
index_profiles = importlib.util.module_from_spec(spec)
spec.loader.exec_module(index_profiles)
PROFILES = [
    profile for profile in index_profiles.PROFILES
    if profile[0] in {"amazon-fts2", "nyc-fts3", "leipzig-fts1", "musenalm-fts4"}
]


def run_json(command, environment=None):
    result = subprocess.run(
        command, cwd=ROOT, env=environment, capture_output=True, text=True, check=True
    )
    return json.loads(result.stdout)


def build_command(profile, variant):
    _, repository, collection, encoding, fields = profile
    command = [
        str(ROOT / "target/release/index-build-profile"),
        "--repo", str(ROOT / "examples" / repository),
        "--collection", collection,
        "--encoding", encoding,
        "--skip-snapshot",
    ]
    if variant == "combined":
        command.append("--combined-inventory")
    for field in fields:
        command.extend(("--field", field))
    return command


def io_command(profile, backend, workers, depth):
    _, repository, collection, _, _ = profile
    command = [
        str(ROOT / "target/release/index_io_benchmark"),
        "--repo", str(ROOT / "examples" / repository),
        "--collection", collection,
        "--backend", backend,
    ]
    if workers:
        command.extend(("--workers", str(workers)))
    if depth:
        command.extend(("--depth", str(depth)))
    return command


def variants(phase):
    if phase == "build":
        return [("current-rayon", 32, 0), ("combined", 32, 0)]
    if phase == "metadata":
        return (
            [("rayon-metadata", workers, 0) for workers in (8, 16, 32)]
            + [("sync-statx", workers, 0) for workers in (8, 16, 32)]
            + [("uring-statx", 0, depth) for depth in (32, 64, 128, 256, 512)]
        )
    return (
        [("read-rayon", workers, 0) for workers in (8, 16, 32)]
        + [("read-uring", 0, depth) for depth in (16, 32, 64, 128, 256)]
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--phase", choices=("build", "metadata", "read"), required=True)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--only", help="comma-separated profile names")
    parser.add_argument("--output", type=Path, default=ROOT / "crates/bench/results/index-io-benchmark-2026-11-03.ndjson")
    parser.add_argument("--append", action="store_true")
    parser.add_argument("--no-build", action="store_true")
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    requested = set(args.only.split(",")) if args.only else None
    profiles = [p for p in PROFILES if requested is None or p[0] in requested]
    if not profiles or requested and requested - {p[0] for p in profiles}:
        parser.error("unknown or empty profile selection")

    if not args.no_build:
        command = ["cargo", "build", "--release", "-p", "objekte-bench"]
        if args.phase == "build":
            command += ["--features", "index-timings", "--bin", "index-build-profile"]
        else:
            command += ["--bin", "index_io_benchmark"]
        subprocess.run(command, cwd=ROOT, check=True)

    revision = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
    ).strip()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("a" if args.append else "w", encoding="utf-8") as output:
        for round_number in range(1, args.runs + 1):
            ordered_profiles = profiles if round_number % 2 else profiles[::-1]
            ordered_variants = variants(args.phase) if round_number % 2 else variants(args.phase)[::-1]
            for profile in ordered_profiles:
                for backend, workers, depth in ordered_variants:
                    if args.phase == "build":
                        command = build_command(profile, backend)
                        environment = {**os.environ, "RAYON_NUM_THREADS": str(workers)}
                    else:
                        command = io_command(profile, backend, workers, depth)
                        environment = None
                    record = run_json(command, environment)
                    record.update({
                        "phase": args.phase,
                        "profile": profile[0],
                        "round": round_number,
                        "revision": revision,
                    })
                    output.write(json.dumps(record, sort_keys=True) + "\n")
                    output.flush()
                print(f"{args.phase}: {profile[0]} round {round_number}/{args.runs}", flush=True)


if __name__ == "__main__":
    main()
