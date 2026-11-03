#!/usr/bin/env python3
"""Run paired, read-only candidate I/O comparisons with warmed picker indexes."""

import argparse
import json
from pathlib import Path
import platform
import subprocess


ROOT = Path(__file__).resolve().parents[2]
BIN = ROOT / "target/release/candidate_io_benchmark"
PROFILES = {
    "amazon": ("amazon-reviews-2023", "reviews", "excellent", "ascii"),
    "nyc": ("nyc311", "requests", "water", "ascii"),
    "leipzig": ("leipzig-multilingual", "texts", "Liebe", "utf8_bytes"),
    "musenalm": ("musenalm", "contents", "Goethe", "ascii"),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--only", default=",".join(PROFILES))
    parser.add_argument("--runs", type=int, default=7)
    parser.add_argument("--workers", type=int, default=16)
    parser.add_argument("--depths", default="8,32")
    parser.add_argument("--sizes", default="32,256,1024,4096")
    parser.add_argument("--allocations", action="store_true")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--append", action="store_true")
    parser.add_argument(
        "--output", type=Path,
        default=ROOT / "crates/bench/results/candidate-io-benchmark-2026-11-03.ndjson",
    )
    args = parser.parse_args()
    wanted = args.only.split(",")
    if any(name not in PROFILES for name in wanted):
        parser.error("unknown profile")
    if args.runs < 1 or args.workers < 1:
        parser.error("runs and workers must be positive")

    if not args.no_build:
        command = ["cargo", "build", "--release", "-p", "objekte-bench", "--bin", "candidate_io_benchmark"]
        if args.allocations:
            command += ["--features", "candidate-allocations"]
        subprocess.run(command, cwd=ROOT, check=True)

    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    cpu = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                if line.startswith("model name")), "unknown")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("a" if args.append else "w", encoding="utf-8") as output:
        for name in wanted:
            repository, collection, pattern, encoding = PROFILES[name]
            command = [
                str(BIN), "--repo", str(ROOT / "examples" / repository),
                "--collection", collection, "--pattern", pattern, "--encoding", encoding,
                "--runs", str(args.runs), "--workers", str(args.workers),
                "--depths", args.depths, "--sizes", args.sizes,
            ]
            completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, check=True)
            for line in completed.stdout.splitlines():
                record = json.loads(line)
                record.update({
                    "profile": name,
                    "revision": revision,
                    "kernel": platform.release(),
                    "cpu": cpu,
                    "allocations_instrumented": args.allocations,
                    "cache_condition": "largely warm; no cache eviction",
                })
                output.write(json.dumps(record, separators=(",", ":")) + "\n")
            output.flush()
            print(f"{name}: {len(completed.stdout.splitlines())} records")


if __name__ == "__main__":
    main()
