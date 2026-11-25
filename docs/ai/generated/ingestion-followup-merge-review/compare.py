#!/usr/bin/env python3
"""Compare identical managed-file scale fixtures before and after the merge review."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile


def run(command, **kwargs):
    return subprocess.check_output(command, text=True, **kwargs).strip()


def revision(tree):
    return run(["git", "rev-parse", "HEAD"], cwd=tree)


def build(tree, target, name, destination):
    subprocess.run(
        ["cargo", "build", "--locked", "--release", "-p", "objekte-search-core",
         "--features", "detailed-profiling", "--example", name],
        cwd=tree, env={**os.environ, "CARGO_TARGET_DIR": str(target)}, check=True,
    )
    shutil.copy2(target / "release/examples" / name, destination)


def main(args):
    final = args.final.resolve()
    reviewed = args.reviewed.resolve()
    target = args.target.resolve()
    reviewed_target = args.reviewed_target.resolve()
    if reviewed_target == target:
        raise ValueError("Use separate Cargo target directories for the two worktrees")
    evidence = args.output.resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    name = "bench_managed_file_scale"
    reviewed_name = name + "_merge_review_probe"
    relative = Path("crates/core/examples") / (name + ".rs")
    source = (final / relative).read_text()
    reviewed_source, count = re.subn(
        r'    if mixed && view\.startup_stats\(\).build_workers > 1 && large_peak < 2 \{\n'
        r'        return Err\("large registered file did not overlap its own decoders"\.into\(\)\);\n'
        r'    \}\n',
        "", source,
    )
    assert count == 1, "expected the mixed-decoder acceptance assertion"
    baseline_probe = reviewed / relative.with_stem(reviewed_name)
    base_revision = revision(reviewed)
    assert base_revision == "2a748ed285ced6d5980f2cfd4e823a4295bf3c98"
    final_revision = revision(final)
    workloads = [
        ("jsonl", 32),
        ("registered", 1),
        ("registered", 8),
        ("registered-mixed", 1),
        ("registered-mixed", 8),
        ("registered-many", 8),
    ]
    with tempfile.TemporaryDirectory(prefix="objekte-ingestion-review-bench-") as work:
        work = Path(work)
        base_binary = work / "reviewed"
        final_binary = work / "repair"
        with baseline_probe.open("x") as handle:
            handle.write(reviewed_source)
        try:
            build(reviewed, reviewed_target, reviewed_name, base_binary)
        finally:
            baseline_probe.unlink()
        build(final, target, name, final_binary)
        with (evidence / "comparison.jsonl").open("w") as log:
            for mode, workers in workloads:
                for pair in range(args.pairs):
                    for side, binary in [("reviewed", base_binary), ("repair", final_binary)]:
                        print(mode, workers, pair, side, flush=True)
                        command = [str(binary), str(work / f"{mode}-{workers}-{pair}-{side}"), mode, str(workers)]
                        proc = subprocess.run(command, capture_output=True, text=True)
                        row = {"mode": mode, "workers": workers, "pair": pair, "side": side,
                               "command": command, "exit": proc.returncode}
                        row["result"] = json.loads(proc.stdout) if proc.returncode == 0 else proc.stderr
                        log.write(json.dumps(row, separators=(",", ":")) + "\n")
                        log.flush()
                        proc.check_returncode()
                        if side == "reviewed" and mode == "registered-mixed" and workers == 8:
                            if row["result"]["large_file_active_decoders"] != 1:
                                raise ValueError("The reviewed build must decode the mixed large file inline")
        binaries = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                    for path in (base_binary, final_binary)}
    governor = Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
    manifest = {
        "reviewed": base_revision, "repair": final_revision,
        "platform": platform.platform(), "cpus": len(os.sched_getaffinity(0)),
        "governor": governor.read_text().strip() if governor.exists() else None,
        "rustc": run(["rustc", "--version"]), "cargo": run(["cargo", "--version"]),
        "separate_cargo_targets": True,
        "flags": ["--locked", "--release", "--features", "detailed-profiling"],
        "binaries_sha256": binaries, "pairs": args.pairs, "runs": len(workloads) * args.pairs * 2,
        "reviewed_probe_difference": "Observe the mixed file overlap without requiring the repaired behavior",
    }
    (evidence / "comparison-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--final", type=Path, default=Path.cwd())
    parser.add_argument("--reviewed", type=Path, required=True)
    parser.add_argument("--target", type=Path, required=True)
    parser.add_argument("--reviewed-target", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pairs", type=int, default=3)
    main(parser.parse_args())
