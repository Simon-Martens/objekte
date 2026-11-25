#!/usr/bin/env python3
"""Sequential parent/final invocations of existing release benchmarks."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--parent-worktree", type=Path, required=True)
    parser.add_argument("--final-worktree", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    fixture = args.fixture.resolve(strict=True)
    worktrees = {
        "parent": args.parent_worktree.resolve(strict=True),
        "final": args.final_worktree.resolve(strict=True),
    }
    names = ["bench_build_musenalm", "bench_lifecycle_mutations", "bench_native_record_ingestion"]
    manifest = {
        "fixture": str(fixture),
        "implementations": {},
        "runs": [],
    }
    for label, worktree in worktrees.items():
        binaries = {name: worktree / "target/release/examples" / name for name in names}
        manifest["implementations"][label] = {
            "commit": subprocess.check_output(["git", "-C", str(worktree), "rev-parse", "HEAD"], text=True).strip(),
            "binaries": {name: {"path": str(binary), "sha256": digest(binary)} for name, binary in binaries.items()},
        }

    def run(label, side, name, arguments, environment=None):
        worktree = worktrees[side]
        binary = worktree / "target/release/examples" / name
        command = [str(binary), *map(str, arguments)]
        result = subprocess.run(command, cwd=worktree, capture_output=True, text=True, env=environment)
        (out / f"{label}.stdout").write_text(result.stdout)
        (out / f"{label}.stderr").write_text(result.stderr)
        row = {
            "label": label,
            "side": side,
            "command": command,
            "exit_status": result.returncode,
            "stdout_sha256": digest(out / f"{label}.stdout"),
            "stderr_sha256": digest(out / f"{label}.stderr"),
        }
        if result.returncode == 0 and name != "bench_lifecycle_mutations":
            row["observation"] = json.loads(result.stdout)
        manifest["runs"].append(row)
        (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"{label}: {result.returncode}", flush=True)
        if result.returncode:
            raise RuntimeError(result.stderr)

    import os

    with tempfile.TemporaryDirectory(prefix="objekte-managed-comparison-") as scratch_name:
        scratch = Path(scratch_name)
        for alphabet in ["ascii", "unicode"]:
            for sample in range(1, 4):
                for side in ["parent", "final"]:
                    label = f"build-{alphabet}-{sample}-{side}"
                    run(label, side, "bench_build_musenalm", ["--root", fixture, "--alphabet", alphabet, "--build-workers", "32"])
        for sample in range(1, 4):
            for side in ["parent", "final"]:
                label = f"lifecycle-small-{sample}-{side}"
                root = scratch / label
                run(label, side, "bench_lifecycle_mutations", ["small", root], {**os.environ, "OBJEKTE_BENCH_MUTATE_COPY": "1"})
        for batch_size, mode in [(1, "normal"), (256, "normal"), (256, "overflow")]:
            for sample in range(1, 4):
                for side in ["parent", "final"]:
                    label = f"direct-{batch_size}-{mode}-{sample}-{side}"
                    root = scratch / label
                    root.mkdir()
                    run(label, side, "bench_native_record_ingestion", ["ingest", batch_size, 1, mode, 65536, root])
        for side in ["parent", "final"]:
            label = f"routing-{side}"
            run(label, side, "bench_native_record_ingestion", ["routing", 64, 65536, scratch / label])
    (out / "completed").write_text("complete\n")


if __name__ == "__main__":
    main()
