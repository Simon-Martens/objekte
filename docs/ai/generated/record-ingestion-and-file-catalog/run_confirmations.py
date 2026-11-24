#!/usr/bin/env python3
"""Interleave release binaries to confirm a paired performance screen."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


HISTORICAL_OUTLIERS = {
    "ascii": "Q029,Q031,Q038,Q063,Q079,Q086,Q088,Q090,Q097,Q101,Q104,Q106,Q112,Q115,Q128,Q137,Q140,Q142,Q143,Q144",
    "unicode": "Q039,Q050,Q051,Q056,Q061,Q091,Q108,Q109,Q110,Q124",
}


def digest(path):
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(block)
    return checksum.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline-bin", type=Path, required=True)
    parser.add_argument("--after-bin", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--baseline-worktree", type=Path, required=True)
    parser.add_argument("--after-worktree", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--phases", default="historical-ascii,historical-unicode,lifecycle-small")
    args = parser.parse_args()
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=False)
    fixture = args.fixture.resolve(strict=True)
    files = sorted(fixture.rglob("*.json"))
    sides = {
        "before": (args.baseline_bin.resolve(strict=True), args.baseline_worktree.resolve(strict=True)),
        "after": (args.after_bin.resolve(strict=True), args.after_worktree.resolve(strict=True)),
    }
    manifest = {
        "fixture": str(fixture),
        "pairs": 3,
        "first_side": ["before", "after", "before"],
        "sides": {
            side: {
                "sha": subprocess.check_output(["git", "-C", str(worktree), "rev-parse", "HEAD"], text=True).strip(),
                "build_binary_sha256": digest(binary / "bench_build_musenalm"),
                "query_binary_sha256": digest(binary / "bench_musenalm"),
                "lifecycle_binary_sha256": digest(binary / "bench_lifecycle_mutations"),
            }
            for side, (binary, worktree) in sides.items()
        },
        "invocations": [],
    }
    workers = min(32, len(os.sched_getaffinity(0)))

    def invoke(label, side, command, environment=None):
        for path in files:
            with path.open("rb") as stream:
                while stream.read(1024 * 1024):
                    pass
        directory = output / label
        directory.mkdir(parents=True)
        stdout = directory / "stdout.log"
        stderr = directory / "stderr.log"
        with stdout.open("wb") as out, stderr.open("wb") as err:
            status = subprocess.run(command, cwd=sides[side][1], stdout=out, stderr=err,
                                    env={**os.environ, **(environment or {})}).returncode
        manifest["invocations"].append({
            "label": label, "side": side, "command": list(map(str, command)),
            "exit_status": status, "stdout_sha256": digest(stdout), "stderr_sha256": digest(stderr),
        })
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"{label}: exit {status}", flush=True)
        if status:
            raise RuntimeError(f"{label} failed; see {stderr}")

    for phase in args.phases.split(","):
        if phase not in ("build-ascii", "historical-ascii", "historical-unicode", "lifecycle-small"):
            raise ValueError(f"unknown phase: {phase}")
        for pair in range(1, 4):
            order = ("before", "after") if pair % 2 else ("after", "before")
            for side in order:
                binary = sides[side][0]
                label = f"{phase}-pair-{pair}-{side}"
                if phase == "build-ascii":
                    command = [binary / "bench_build_musenalm", "--root", fixture,
                               "--alphabet", "ascii", "--build-workers", str(workers)]
                    environment = None
                elif phase.startswith("historical"):
                    alphabet = phase.removeprefix("historical-")
                    result = output / label / "results"
                    command = [binary / "bench_musenalm", "--root", fixture,
                               "--profiles", f"{alphabet}-off", "--verified-cache-bytes", "0",
                               "--query-workers", "1", "--cases", HISTORICAL_OUTLIERS[alphabet],
                               "--repetitions", "7", "--result-dir", result]
                    environment = None
                else:
                    command = [binary / "bench_lifecycle_mutations", "small",
                               output / label / "mutation-fixture"]
                    environment = {"OBJEKTE_BENCH_MUTATE_COPY": "1"}
                invoke(label, side, command, environment)
    (output / "completed").write_text("complete\n")


if __name__ == "__main__":
    main()
