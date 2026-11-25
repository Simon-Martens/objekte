#!/usr/bin/env python3
"""Run a sequential paired comparison using the repository benchmark examples."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import tempfile


def command(args, cwd=None, env=None):
    return subprocess.run(args, cwd=cwd, env=env, capture_output=True, text=True)


def require(result):
    if result.returncode:
        raise RuntimeError(f"{result.args}: {result.stderr}\n{result.stdout}")
    return result.stdout


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--pairs", type=int, default=3)
    args = parser.parse_args()
    roots = {"base": args.baseline.resolve(), "final": args.candidate.resolve()}
    fixture = args.fixture.resolve(strict=True)
    out = args.out.resolve()
    observations = []
    manifest = {
        "fixture": str(fixture),
        "platform": platform.platform(),
        "available_cpus": len(os.sched_getaffinity(0)),
        "rustc": require(command(["rustc", "--version"])).strip(),
        "cargo": require(command(["cargo", "--version"])).strip(),
        "implementations": {},
    }
    examples = ("bench_native_record_ingestion", "bench_lifecycle_mutations")
    common_metadata = roots["base"] / (
        "docs/ai/generated/core-managed-file-ingestion/metadata_query_benchmark.rs"
    )
    candidate_examples = roots["final"] / "crates/core/examples"
    common_build = (candidate_examples / "bench_build_musenalm.rs").read_bytes()
    common_scale = (candidate_examples / "bench_managed_file_scale.rs").read_bytes()
    for side, root in roots.items():
        names = [*examples, "bench_metadata_repair_probe"]
        ephemeral = {
            "bench_metadata_repair_probe": common_metadata.read_bytes(),
        }
        if side == "base":
            # The baseline is expected to produce different segment packing.
            # Keep all source input, identity, locator and read assertions.
            old_compatible = common_scale.replace(
                b"if segment_counts != expected_segments",
                b"if (false && segment_counts != expected_segments)",
            )
            ephemeral["bench_managed_file_scale_repair_probe"] = old_compatible
            ephemeral["bench_build_musenalm_repair_probe"] = common_build
            names += list(ephemeral)[1:]
        else:
            names += ["bench_managed_file_scale", "bench_build_musenalm"]
        build = [
            "cargo", "build", "--locked", "--release", "-p", "objekte-search-core",
            "--features", "detailed-profiling",
        ]
        for name in names:
            build += ["--example", name]
        created = []
        try:
            for name, content in ephemeral.items():
                file = root / "crates/core/examples" / f"{name}.rs"
                with file.open("xb") as handle:
                    created.append(file)
                    handle.write(content)
            require(command(build, cwd=root))
        finally:
            for file in created:
                file.unlink()
        binaries = {name: root / "target/release/examples" / name for name in names}
        manifest["implementations"][side] = {
            "revision": require(command(["git", "rev-parse", "HEAD"], cwd=root)).strip(),
            "build": build,
            "binaries": {name: sha(binary) for name, binary in binaries.items()},
            "copied_sources": {
                name: hashlib.sha256(content).hexdigest()
                for name, content in ephemeral.items()
            },
        }

    def save():
        out.write_text(json.dumps(manifest, indent=2) + "\n")
        out.with_suffix(".jsonl").write_text(
            "".join(json.dumps(row, separators=(",", ":")) + "\n" for row in observations)
        )

    def run(workload, sample, side, binary, arguments, kind="json", env=None):
        cwd = roots[side]
        cmd = [str(cwd / "target/release/examples" / binary), *map(str, arguments)]
        result = command(cmd, cwd=cwd, env=env)
        row = {"workload": workload, "sample": sample, "side": side, "command": cmd,
               "exit_status": result.returncode}
        if result.returncode:
            row.update(stdout=result.stdout, stderr=result.stderr)
            observations.append(row)
            save()
            require(result)
        if kind == "json":
            row["result"] = json.loads(result.stdout)
        elif kind == "metadata":
            row["result"] = {
                key: int(value) for key, value in re.findall(r"(\w+)=(\d+)", result.stdout)
            }
        else:
            row["result"] = result.stdout
        observations.append(row)
        save()
        print(f"{workload} {sample} {side}", flush=True)

    with tempfile.TemporaryDirectory(prefix="objekte-ingestion-repair-bench-") as scratch:
        scratch = Path(scratch)
        for alphabet in ("ascii", "unicode"):
            for i in range(args.pairs):
                for side in roots:
                    binary = "bench_build_musenalm" + ("_repair_probe" if side == "base" else "")
                    run(f"build-{alphabet}", i, side, binary,
                        ["--root", fixture, "--alphabet", alphabet, "--build-workers", 32])
        for size, producers, mode in ((1, 1, "normal"), (256, 1, "normal"), (256, 4, "overflow")):
            for i in range(args.pairs):
                for side in roots:
                    directory = scratch / f"ingest-{size}-{mode}-{i}-{side}"
                    directory.mkdir()
                    run(f"ingest-{size}-{mode}", i, side, "bench_native_record_ingestion",
                        ["ingest", size, producers, mode, 65536, directory])
        for mode, workers in (("jsonl", 32), ("registered", 1), ("registered", 8)):
            for i in range(args.pairs):
                for side in roots:
                    binary = "bench_managed_file_scale" + ("_repair_probe" if side == "base" else "")
                    run(f"scale-{mode}-{workers}", i, side, binary,
                        [scratch / f"scale-{mode}-{workers}-{i}-{side}", mode, workers])
        for i in range(args.pairs):
            for side in roots:
                run("metadata", i, side, "bench_metadata_repair_probe",
                    [scratch / f"metadata-{i}-{side}"], kind="metadata")
                run("lifecycle", i, side, "bench_lifecycle_mutations",
                    ["small", scratch / f"lifecycle-{i}-{side}"], kind="table",
                    env={**os.environ, "OBJEKTE_BENCH_MUTATE_COPY": "1"})
        for side in roots:
            run("routing", 0, side, "bench_native_record_ingestion",
                ["routing", 64, 65536, scratch / f"routing-{side}"])
    manifest["complete"] = True
    manifest["run_count"] = len(observations)
    save()


if __name__ == "__main__":
    main()
