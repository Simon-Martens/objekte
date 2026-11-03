#!/usr/bin/env python3
"""Prepare snapshots once, then profile isolated restore processes repeatedly."""

import argparse
import json
from pathlib import Path
import subprocess

from run_index_build_profiles import PROFILES, ROOT


BENCH = ROOT / "target/release/index-build-profile"


def command(profile, snapshot):
    _, repo_name, collection, encoding, fields = profile
    result = [str(BENCH), "--repo", str(ROOT / "examples" / repo_name),
              "--collection", collection, "--encoding", encoding,
              "--snapshot-path", str(snapshot)]
    for field in fields:
        result += ["--field", field]
    return result


def run(args):
    result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True, check=True)
    return json.loads(result.stdout)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--only", help="comma-separated profile names")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--snapshots", type=Path, default=ROOT / "target/snapshot-restore")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--reuse-snapshots", action="store_true")
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    wanted = set(args.only.split(",")) if args.only else None
    profiles = [p for p in PROFILES if wanted is None or p[0] in wanted]
    if not profiles or (wanted and wanted - {p[0] for p in profiles}):
        parser.error("unknown or empty profile selection")
    if not args.no_build:
        subprocess.run(["cargo", "build", "--release", "-p", "objekte-bench",
                        "--features", "index-timings", "--bin", "index-build-profile"],
                       cwd=ROOT, check=True)
    args.snapshots.mkdir(parents=True, exist_ok=True)
    for profile in profiles:
        name = profile[0]
        snapshot = args.snapshots / f"{name}.bigram"
        if not args.reuse_snapshots or not snapshot.is_file():
            print(f"preparing {name}", flush=True)
            run(command(profile, snapshot))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", encoding="utf-8") as output:
        for round_number in range(1, args.runs + 1):
            for profile in (profiles if round_number % 2 else list(reversed(profiles))):
                name = profile[0]
                snapshot = args.snapshots / f"{name}.bigram"
                record = run(command(profile, snapshot) + ["--restore-only"])
                record.update({"profile": name, "round": round_number})
                output.write(json.dumps(record, sort_keys=True) + "\n")
                output.flush()
                print(f"{name} {round_number}/{args.runs}: "
                      f"{record['snapshot_restore_wall_nanos']/1e6:.1f} ms", flush=True)


if __name__ == "__main__":
    main()
