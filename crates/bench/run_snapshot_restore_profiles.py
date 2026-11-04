#!/usr/bin/env python3
"""Prepare snapshots once, then profile isolated restore processes repeatedly."""

import argparse
import json
from pathlib import Path
import subprocess

from run_index_build_profiles import PROFILES, ROOT


BENCH = ROOT / "target/release/index-build-profile"


def command(profile, snapshot, binary):
    _, repo_name, collection, encoding, fields = profile
    result = [str(binary), "--repo", str(ROOT / "examples" / repo_name),
              "--collection", collection, "--encoding", encoding,
              "--snapshot-path", str(snapshot)]
    for field in fields:
        result += ["--field", field]
    return result


def run(args, evidence_dir=None, label=None):
    result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
    if evidence_dir is not None:
        (evidence_dir / f"{label}.stdout.json").write_text(result.stdout)
        (evidence_dir / f"{label}.stderr.log").write_text(result.stderr)
        (evidence_dir / f"{label}.command.json").write_text(json.dumps({
            "command": args, "exit_status": result.returncode,
        }, indent=2) + "\n")
    result.check_returncode()
    return json.loads(result.stdout)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--only", help="comma-separated profile names")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--snapshots", type=Path, default=ROOT / "target/snapshot-restore")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--reuse-snapshots", action="store_true")
    parser.add_argument("--bin-dir", type=Path, default=ROOT / "target/release")
    parser.add_argument("--evidence-dir", type=Path)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    wanted = set(args.only.split(",")) if args.only else None
    profiles = [p for p in PROFILES if wanted is None or p[0] in wanted]
    if not profiles or (wanted and wanted - {p[0] for p in profiles}):
        parser.error("unknown or empty profile selection")
    if not args.no_build:
        subprocess.run(["cargo", "build", "--release", "-p", "objekte-bench",
                        "--features", "index-timings,allocation-profile", "--bin", "index-build-profile"],
                       cwd=ROOT, check=True)
    binary = args.bin_dir / "index-build-profile"
    if args.evidence_dir is not None:
        args.evidence_dir.mkdir(parents=True, exist_ok=True)
    args.snapshots.mkdir(parents=True, exist_ok=True)
    for profile in profiles:
        name = profile[0]
        snapshot = args.snapshots / f"{name}.bigram"
        if not args.reuse_snapshots or not snapshot.is_file():
            print(f"preparing {name}", flush=True)
            run(command(profile, snapshot, binary), args.evidence_dir, f"{name}-prepare")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", encoding="utf-8") as output:
        for round_number in range(1, args.runs + 1):
            for profile in (profiles if round_number % 2 else list(reversed(profiles))):
                name = profile[0]
                snapshot = args.snapshots / f"{name}.bigram"
                record = run(command(profile, snapshot, binary) + ["--restore-only"],
                             args.evidence_dir, f"{name}-round{round_number}")
                record.update({"profile": name, "round": round_number})
                output.write(json.dumps(record, sort_keys=True) + "\n")
                output.flush()
                print(f"{name} {round_number}/{args.runs}: "
                      f"{record['snapshot_restore_wall_nanos']/1e6:.1f} ms", flush=True)


if __name__ == "__main__":
    main()
