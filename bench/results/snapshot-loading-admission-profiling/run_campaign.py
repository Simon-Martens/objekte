#!/usr/bin/env python3
"""Sequential matched B0/B1/B2/B3 warm-cache trial campaign."""

import hashlib
import json
import subprocess
from pathlib import Path

BASE = Path(__file__).resolve().parent
ROOT = Path("/home/simon/source/objekte/examples/musenalm")
WORKTREE = BASE.parents[2]
VARIANTS = {
    "b0": "6c808ebb70203d2442a5e8f2f595199ae5f8d171",
    "b1": "7ac10f36cd762708aa943a70fa4700ffb9592d5c",
    "b2": "55a38262b28af74cb5858c4f4478385ff4b15804",
    "b3": "93a98cc4dc578dde768c3a83e41b97d36f33e9b2",
}
SNAPSHOT = next((BASE / "fixture").glob("trial-*/index.snapshot"))


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


snapshot_hash = sha256(SNAPSHOT)
binary_hashes = {name: sha256(BASE / "local-binaries" / name) for name in VARIANTS}


def run(kind, variant, round_number, profiled=False, alphabet="ascii", snapshot=SNAPSHOT):
    prefix = "" if alphabet == "ascii" else f"{alphabet}-"
    name = f"{prefix}{kind}-{variant}-{'on' if profiled else 'off'}-{round_number:02d}"
    output = BASE / "runs" / name
    binary = BASE / "local-binaries" / variant
    args = [
        "taskset", "-c", "0-31", str(binary), "--root", str(ROOT),
        "--alphabet", alphabet, "--workers", "32",
    ]
    if kind == "construction":
        args += ["--incremental", "--check-queries"]
    else:
        args += ["--restore-snapshot", str(snapshot)]
    if profiled:
        args.append("--profile")
    args += ["--result-dir", str(output)]
    completed = subprocess.run(args, cwd=WORKTREE, text=True, capture_output=True, check=True)
    result = json.loads(completed.stdout.strip())
    trial = WORKTREE / result["result_dir"] if not Path(result["result_dir"]).is_absolute() else Path(result["result_dir"])
    sample = json.loads((trial / "sample.json").read_text())
    if kind == "restore" and sample.get("restored_from_cache") is not True:
        raise RuntimeError(f"{name} did not restore from cache")
    meta = {
        "name": name,
        "kind": kind,
        "variant": variant,
        "source_commit": VARIANTS[variant],
        "binary_sha256": binary_hashes[variant],
        "snapshot_sha256": snapshot_hash if snapshot == SNAPSHOT else sha256(snapshot),
        "profiled": profiled,
        "alphabet": alphabet,
        "exact_command": args,
        "runner_checkout_revision": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=WORKTREE, text=True
        ).strip(),
    }
    (trial / "trial-meta.json").write_text(json.dumps(meta, indent=2, sort_keys=True) + "\n")
    elapsed = sample["build_ns" if kind == "construction" else "load_ns"] / 1e6
    print(f"{name} {elapsed:.2f} ms", flush=True)


if __name__ == "__main__":
    for round_number in range(1, 8):
        order = list(VARIANTS)
        order = order[(round_number - 1) % 4:] + order[:(round_number - 1) % 4]
        for variant in order:
            run("restore", variant, round_number)
        if round_number % 2:
            run("construction", "b0", round_number)
            run("construction", "b3", round_number, profiled=True)
            run("construction", "b3", round_number)
        else:
            run("construction", "b3", round_number)
            run("construction", "b3", round_number, profiled=True)
            run("construction", "b0", round_number)
        if round_number <= 3:
            for variant in (("b0", "b3") if round_number % 2 else ("b3", "b0")):
                run("restore", variant, round_number, profiled=True)
