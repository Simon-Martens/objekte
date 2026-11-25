#!/usr/bin/env python3
"""Run the frozen file-backed before matrix for record ingestion work.

Measurement-only driver. Invoke from either measured source worktree and pass
its already-built release example directory, the canonical fixture, and a fresh
output directory. Source tree and fixture are never changed by this driver.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fixture_files(root):
    return sorted(path for path in root.rglob("*.json") if path.is_file())


def fingerprint(root, files):
    content = hashlib.sha256()
    metadata = hashlib.sha256()
    for path in files:
        relative = path.relative_to(root).as_posix().encode()
        stat = path.stat()
        metadata.update(relative + b"\0")
        metadata.update(f"{stat.st_size}:{stat.st_mtime_ns}".encode() + b"\n")
        content.update(relative + b"\0")
        content.update(bytes.fromhex(sha256(path)))
    return {"json_files": len(files), "content_sha256": content.hexdigest(),
            "metadata_sha256": metadata.hexdigest()}


def warm(files):
    for path in files:
        with path.open("rb") as stream:
            while stream.read(1024 * 1024):
                pass


def command_output(*command):
    return subprocess.check_output(command, text=True).strip()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--worktree", type=Path, required=True)
    args = parser.parse_args()
    source = args.worktree.resolve(strict=True)
    fixture = args.fixture.resolve(strict=True)
    binaries = args.bin.resolve(strict=True)
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=False)
    files = fixture_files(fixture)
    affinity = sorted(os.sched_getaffinity(0))
    workers = min(32, len(affinity))
    governors = sorted({p.read_text().strip() for p in
                        Path("/sys/devices/system/cpu").glob("cpu[0-9]*/cpufreq/scaling_governor")})
    workload = source / "crates/cli/benches"
    if not workload.is_dir():
        workload = source / "crates/core/benches"
    manifest = {
        "base_commit": command_output("git", "-C", str(source), "rev-parse", "HEAD"),
        "worktree": str(source), "fixture": str(fixture),
        "fixture_fingerprint": fingerprint(fixture, files),
        "cargo_lock_sha256": sha256(source / "Cargo.lock"),
        "historical_manifest_sha256": sha256(workload / "musenalm_queries.json"),
        "extended_manifest_sha256": sha256(workload / "musenalm_queries_extended.json"),
        "rustc": command_output("rustc", "--version"),
        "cargo": command_output("cargo", "--version"),
        "affinity": affinity, "governors": governors,
        "workers": workers, "query_workers": 1,
        "rustflags": os.environ.get("RUSTFLAGS", ""),
        "allocator": "Rust release default (system)",
        "binaries": {name: {"path": str(binaries / name), "sha256": sha256(binaries / name)}
                     for name in ("bench_build_musenalm", "bench_musenalm", "bench_lifecycle_mutations")},
        "invocations": [],
    }
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

    def invoke(label, command, env=None):
        # The entire source corpus is read once before each process so cache
        # warming is outside the benchmark's own clock.
        warm(files)
        phase = output / label
        phase.mkdir(parents=True)
        stdout_path = phase / "stdout.log"
        stderr_path = phase / "stderr.log"
        started = time.perf_counter_ns()
        with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
            process = subprocess.run(command, cwd=source, stdout=stdout, stderr=stderr,
                                     env={**os.environ, **(env or {})})
        record = {"label": label, "command": list(map(str, command)),
                  "exit_status": process.returncode,
                  "driver_elapsed_ns": time.perf_counter_ns() - started,
                  "stdout_sha256": sha256(stdout_path), "stderr_sha256": sha256(stderr_path)}
        manifest["invocations"].append(record)
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"{label}: exit {process.returncode}", flush=True)
        if process.returncode:
            raise RuntimeError(f"{label} failed; see {stderr_path}")

    focused = "Q001,Q030,Q034,Q035,Q054,Q081,Q116,Q136,Q144"
    extended = "Q155,Q163,Q166,Q170,Q195,Q219"
    build = binaries / "bench_build_musenalm"
    query = binaries / "bench_musenalm"
    lifecycle = binaries / "bench_lifecycle_mutations"
    for alphabet in ("ascii", "unicode"):
        for sample in range(1, 4):
            invoke(f"build-{alphabet}-{sample}",
                   [build, "--root", fixture, "--alphabet", alphabet,
                    "--build-workers", str(workers)])
    for alphabet in ("ascii", "unicode"):
        profile = f"{alphabet}-off"
        for phase, cases, repetitions in (("historical", None, 1),
                                          ("focused", focused, 7)):
            label = f"{phase}-{alphabet}"
            result = output / label / "results"
            command = [query, "--root", fixture, "--profiles", profile,
                       "--verified-cache-bytes", "0", "--query-workers", "1",
                       "--repetitions", str(repetitions), "--result-dir", result]
            if cases:
                command.extend(["--cases", cases])
            invoke(label, command)
    label = "extended-unicode"
    invoke(label, [query, "--extended", "--root", fixture,
                   "--profiles", "unicode-off", "--verified-cache-bytes", "0",
                   "--query-workers", "1", "--cases", extended,
                   "--repetitions", "7", "--result-dir", output / label / "results"])
    for sample in range(1, 4):
        label = f"lifecycle-small-{sample}"
        invoke(label, [lifecycle, "small", output / label / "mutation-fixture"],
               {"OBJEKTE_BENCH_MUTATE_COPY": "1"})
    (output / "completed").write_text("complete\n")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(error, file=sys.stderr)
        raise SystemExit(1)
