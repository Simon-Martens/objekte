#!/usr/bin/env python3
"""Paired, warm-filesystem index-build and snapshot-restore measurements."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "crates/bench"))
from run_index_build_profiles import PROFILES  # noqa: E402


def command(binary: Path, profile: tuple, *extra: str) -> list[str]:
    _, repository, collection, encoding, fields = profile
    argv = [
        "taskset", "-c", "0-15", str(binary), "--repo",
        str(ROOT / "examples" / repository), "--collection", collection,
        "--encoding", encoding,
    ]
    for field in fields:
        argv.extend(("--field", field))
    return [*argv, *extra]


def run(argv: list[str], path: Path) -> dict:
    result = subprocess.run(
        argv, cwd=ROOT, env=dict(os.environ, RAYON_NUM_THREADS="16"),
        capture_output=True, text=True, check=False,
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.with_suffix(".stdout.json").write_text(result.stdout)
    path.with_suffix(".stderr.log").write_text(result.stderr)
    path.with_suffix(".command.json").write_text(json.dumps({
        "command": argv, "exit_status": result.returncode,
        "rayon_threads": 16, "affinity": "0-15",
    }, indent=2) + "\n")
    if result.returncode:
        raise RuntimeError(f"{path}: exited {result.returncode}: {result.stderr}")
    return json.loads(result.stdout)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--master-bin", type=Path, required=True)
    parser.add_argument("--current-bin", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=5)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    binaries = {
        "master": args.master_bin.resolve(strict=True),
        "current": args.current_bin.resolve(strict=True),
    }
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    snapshots = output / "snapshots"
    snapshots.mkdir(exist_ok=True)

    # Prepare revision-specific snapshots before timing. The timed build path
    # stops before snapshot I/O; the restore path reads the prepared snapshot.
    for profile in PROFILES:
        name = profile[0]
        for label, binary in binaries.items():
            snapshot = snapshots / f"{name}-{label}.bigram"
            print(f"prepare {name} {label}", flush=True)
            run(command(binary, profile, "--snapshot-path", str(snapshot)),
                output / "raw" / "prepare" / f"{name}-{label}")

    for operation in ("build", "restore"):
        rows = []
        for round_number in range(1, args.runs + 1):
            profiles = PROFILES if round_number % 2 else list(reversed(PROFILES))
            for position, profile in enumerate(profiles):
                name = profile[0]
                labels = ("master", "current") if (round_number + position) % 2 else ("current", "master")
                for label in labels:
                    binary = binaries[label]
                    if operation == "build":
                        argv = command(binary, profile, "--skip-snapshot")
                    else:
                        snapshot = snapshots / f"{name}-{label}.bigram"
                        argv = command(binary, profile, "--snapshot-path", str(snapshot), "--restore-only")
                    print(f"{operation} {name} {round_number}/{args.runs} {label}", flush=True)
                    record = run(argv, output / "raw" / operation / f"{name}-round{round_number}-{label}")
                    record.update(profile=name, round=round_number, revision=label)
                    rows.append(record)
                    with (output / f"{operation}.ndjson").open("a") as stream:
                        stream.write(json.dumps(record, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
