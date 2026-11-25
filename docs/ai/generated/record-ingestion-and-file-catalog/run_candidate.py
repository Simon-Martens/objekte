#!/usr/bin/env python3
"""Repeat the frozen 14-invocation matrix on one exact implementation SHA."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time


def digest(path):
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(block)
    return checksum.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--worktree", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--probe-fixture", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    before = json.loads((args.before.resolve(strict=True) / "manifest.json").read_text())
    worktree = args.worktree.resolve(strict=True)
    fixture = args.fixture.resolve(strict=True)
    probe_fixture = args.probe_fixture.resolve(strict=True)
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=False)
    commit = subprocess.check_output(["git", "-C", str(worktree), "rev-parse", "HEAD"],
                                     text=True).strip()
    if str(fixture) != before["fixture"]:
        raise ValueError("fixture path differs from frozen baseline")
    fixture_checkout = fixture.parent.parent
    if subprocess.check_output(["git", "-C", str(fixture_checkout), "rev-parse", "HEAD"],
                               text=True).strip() != before["base_commit"]:
        raise ValueError("fixture checkout is not the frozen baseline")
    if subprocess.check_output(["git", "-C", str(fixture_checkout), "status",
                                "--porcelain", "--", "examples/musenalm"], text=True).strip():
        raise ValueError("fixture differs from its tracked baseline")
    for name in ("rustc", "cargo"):
        observed = subprocess.check_output([name, "--version"], text=True).strip()
        if observed != before[name]:
            raise ValueError(f"{name} differs from frozen baseline")
    if os.environ.get("RUSTFLAGS", "") != before["rustflags"]:
        raise ValueError("RUSTFLAGS differs from frozen baseline")
    if digest(worktree / "Cargo.lock") != before["cargo_lock_sha256"]:
        raise ValueError("Cargo.lock differs from baseline")
    if digest(worktree / "crates/core/benches/musenalm_queries.json") != before["historical_manifest_sha256"]:
        raise ValueError("historical workload differs")
    if digest(worktree / "crates/core/benches/musenalm_queries_extended.json") != before["extended_manifest_sha256"]:
        raise ValueError("extended workload differs")
    if sorted(os.sched_getaffinity(0)) != before["affinity"]:
        raise ValueError("CPU affinity differs")
    files = sorted(fixture.rglob("*.json"))
    if len(files) != before["fixture_fingerprint"]["json_files"]:
        raise ValueError("fixture JSON count differs")
    binary_dir = worktree / "target/release/examples"
    binaries = {
        name: {"path": str(binary_dir / name), "sha256": digest(binary_dir / name)}
        for name in ("bench_build_musenalm", "bench_musenalm", "bench_lifecycle_mutations")
    }
    manifest = {key: before[key] for key in (
        "fixture", "fixture_fingerprint", "cargo_lock_sha256",
        "historical_manifest_sha256", "extended_manifest_sha256",
        "rustc", "cargo", "affinity", "governors", "workers",
        "query_workers", "rustflags", "allocator",
    )}
    manifest.update(base_commit=commit, worktree=str(worktree), binaries=binaries,
                    invocations=[])

    def run(label, command, environment=None):
        directory = output / label
        directory.mkdir()
        stdout, stderr = directory / "stdout.log", directory / "stderr.log"
        start = time.perf_counter_ns()
        with stdout.open("wb") as out, stderr.open("wb") as err:
            status = subprocess.run(command, cwd=worktree, stdout=out, stderr=err,
                                    env={**os.environ, **(environment or {})}).returncode
        manifest["invocations"].append({
            "label": label, "command": list(map(str, command)),
            "exit_status": status, "driver_elapsed_ns": time.perf_counter_ns() - start,
            "stdout_sha256": digest(stdout), "stderr_sha256": digest(stderr),
        })
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"{label}: exit {status}", flush=True)
        if status:
            raise RuntimeError(f"{label} failed: {stderr}")

    for entry in before["invocations"]:
        label = entry["label"]
        command = list(entry["command"])
        command[0] = str(binary_dir / Path(command[0]).name)
        if "--result-dir" in command:
            command[command.index("--result-dir") + 1] = str(output / label / "results")
        if label.startswith("lifecycle-small-"):
            command[2] = str(output / label / "mutation-fixture")
        for path in files:
            with path.open("rb") as source:
                while source.read(1024 * 1024):
                    pass
        run(label, command, {"OBJEKTE_BENCH_MUTATE_COPY": "1"} if
            label.startswith("lifecycle-small-") else None)

    probe_binary = binary_dir / "bench_record_ingestion_gate"
    for label, command in (
        ("direct-probe", [probe_binary, "probe", probe_fixture]),
        ("canonical-ascii", [probe_binary, "canonical", fixture, "ascii"]),
        ("canonical-unicode", [probe_binary, "canonical", fixture, "unicode"]),
        ("canonical-extended-unicode", [probe_binary, "canonical", fixture, "unicode", "extended"]),
    ):
        stdout = output / f"{label}.json"
        stderr = output / f"{label}.stderr"
        with stdout.open("wb") as out, stderr.open("wb") as err:
            status = subprocess.run(command, cwd=worktree, stdout=out, stderr=err).returncode
        if status:
            raise RuntimeError(f"{label} failed: {stderr}")
        json.loads(stdout.read_text())
        print(f"{label}: exit {status}", flush=True)
    manifest["probe_binary_sha256"] = digest(probe_binary)
    manifest["probe_fixture"] = str(probe_fixture)
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (output / "completed").write_text("complete\n")


if __name__ == "__main__":
    main()
