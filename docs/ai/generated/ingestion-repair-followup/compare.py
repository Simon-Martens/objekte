#!/usr/bin/env python3
"""Run paired release probes on the reviewed and follow-up revisions."""

import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile


def git(worktree, *args):
    return subprocess.check_output(["git", "-C", worktree, *args], text=True).strip()


def build(worktree, cargo_target, examples):
    command = [
        "cargo", "build", "--locked", "--release", "-p", "objekte-search-core",
        "--features", "detailed-profiling",
    ]
    for example in examples:
        command.extend(["--example", example])
    subprocess.run(command, cwd=worktree, env={**os.environ, "CARGO_TARGET_DIR": str(cargo_target)}, check=True)


def preserve_binaries(cargo_target, run_dir, side, examples):
    binaries = {}
    for purpose, name in examples.items():
        destination = run_dir / f"{side}-{purpose}"
        shutil.copy2(cargo_target / "release/examples" / name, destination)
        binaries[purpose] = destination
    return binaries


def compare(args):
    final = args.final.resolve()
    reviewed = args.reviewed.resolve()
    cargo_target = args.target.resolve()
    target = git(final, "rev-parse", "HEAD")
    base = git(reviewed, "rev-parse", "HEAD")
    if base != "02cf0a1b2bedf5893e05204c90ade0d55f30c9c5":
        raise ValueError(f"unexpected reviewed revision {base}")

    final_examples = {"scale": "bench_managed_file_scale", "many-json": "bench_build_musenalm"}
    reviewed_examples = {**final_examples, "scale": "bench_managed_file_scale_followup_probe"}
    relative_probe = Path("crates/core/examples/bench_managed_file_scale.rs")
    reviewed_probe = reviewed / relative_probe.with_stem(reviewed_examples["scale"])
    source = (final / relative_probe).read_text()
    reviewed_source = "".join(
        line for line in source.splitlines(keepends=True)
        if '"stream_peak":' not in line
        and "|| populations !=" not in line
        and "|| publications !=" not in line
    )
    if reviewed_probe.exists():
        raise FileExistsError(reviewed_probe)

    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="objekte-followup-bench-") as scratch:
        run_dir = Path(scratch)
        try:
            with reviewed_probe.open("x") as probe:
                probe.write(reviewed_source)
            build(reviewed, cargo_target, reviewed_examples.values())
            base_binaries = preserve_binaries(cargo_target, run_dir, "reviewed", reviewed_examples)
        finally:
            reviewed_probe.unlink(missing_ok=True)

        build(final, cargo_target, final_examples.values())
        final_binaries = preserve_binaries(cargo_target, run_dir, "followup", final_examples)
        workloads = [
            ("large-jsonl-32", "scale", "jsonl", "32"),
            ("large-registered-1", "scale", "registered", "1"),
            ("large-registered-8", "scale", "registered", "8"),
            ("registered-many-8", "scale", "registered-many", "8"),
            ("musenalm-ascii-32", "many-json", "ascii", "32"),
            ("musenalm-unicode-32", "many-json", "unicode", "32"),
        ]
        governor = Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        manifest = {
            "reviewed_revision": base,
            "followup_revision": target,
            "platform": platform.platform(),
            "available_cpus": len(os.sched_getaffinity(0)),
            "cpu0_governor": governor.read_text().strip() if governor.exists() else None,
            "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
            "cargo": subprocess.check_output(["cargo", "--version"], text=True).strip(),
            "cargo_flags": ["--locked", "--release", "--features", "detailed-profiling"],
            "musenalm_fixture": str(args.musenalm.resolve()),
            "pairs_per_workload": args.pairs,
            "reviewed_probe_adjustments": [
                "Omit aggregate stream fields that the reviewed code does not expose",
                "Observe the reviewed registered-many geometry without enforcing the new assertion",
            ],
        }
        with (args.output / "comparison.jsonl").open("w") as samples:
            for name, probe, mode, workers in workloads:
                for pair in range(args.pairs):
                    for side, binaries in [("reviewed", base_binaries), ("followup", final_binaries)]:
                        print(name, pair, side, flush=True)
                        if probe == "scale":
                            fixture = str(run_dir / f"{name}-{pair}-{side}")
                            options = [fixture, mode, workers]
                        else:
                            options = ["--root", str(args.musenalm.resolve()), "--alphabet", mode, "--build-workers", workers]
                        command = [str(binaries[probe]), *options]
                        invocation = subprocess.run(command, capture_output=True, text=True)
                        sample = {
                            "workload": name,
                            "pair": pair,
                            "side": side,
                            "command": command,
                            "returncode": invocation.returncode,
                        }
                        if invocation.returncode == 0:
                            sample["result"] = json.loads(invocation.stdout.strip().splitlines()[-1])
                        else:
                            sample["stderr"] = invocation.stderr
                        samples.write(json.dumps(sample, separators=(",", ":")) + "\n")
                        samples.flush()
                        invocation.check_returncode()
        manifest["samples"] = len(workloads) * args.pairs * 2
        (args.output / "comparison-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reviewed", type=Path, required=True)
    parser.add_argument("--final", type=Path, default=Path.cwd())
    parser.add_argument("--musenalm", type=Path, required=True)
    parser.add_argument("--target", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pairs", type=int, default=3)
    compare(parser.parse_args())
