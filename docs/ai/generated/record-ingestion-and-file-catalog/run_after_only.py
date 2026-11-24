#!/usr/bin/env python3
"""Run the native API measurements on a committed implementation binary."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--worktree", required=True, type=Path)
    args = parser.parse_args()
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=False)
    binary = args.binary.resolve(strict=True)
    worktree = args.worktree.resolve(strict=True)
    manifest = {
        "implementation_sha": subprocess.check_output(
            ["git", "-C", str(worktree), "rev-parse", "HEAD"], text=True
        ).strip(),
        "binary": str(binary),
        "binary_sha256": sha256(binary),
        "seed": 20261124,
        "runs": [],
    }
    variants = [
        (1, 1, "normal"), (1, 4, "normal"),
        (256, 1, "normal"), (256, 4, "normal"),
        (256, 1, "overflow"), (256, 4, "overflow"),
    ]
    with tempfile.TemporaryDirectory(prefix="objekte-native-benchmark-") as temporary:
        scratch = Path(temporary)
        commands = []
        for batch_size, producers, mode in variants:
            for sample in range(1, 4):
                label = f"ingest-{batch_size}-{producers}-{mode}-{sample}"
                parent = scratch / label
                parent.mkdir()
                commands.append((label, [
                    str(binary), "ingest", str(batch_size), str(producers), mode,
                    "65536", str(parent),
                ]))
        for segments in (1, 8, 64):
            for sample in range(1, 4):
                label = f"routing-{segments}-{sample}"
                commands.append((label, [
                    str(binary), "routing", str(segments), "65536",
                    str(scratch / f"{label}-fixture"),
                ]))
        for label, command in commands:
            result = subprocess.run(command, cwd=worktree, capture_output=True, text=True)
            (output / f"{label}.stdout").write_text(result.stdout)
            (output / f"{label}.stderr").write_text(result.stderr)
            observation = json.loads(result.stdout) if result.returncode == 0 else None
            if observation is not None:
                if observation["records"] != 65536:
                    raise RuntimeError(f"{label}: wrong record count")
                if observation["mode"] == "ingest":
                    if observation["accepted_batches"] != observation["published_batches"]:
                        raise RuntimeError(f"{label}: lost admitted batches")
                    if observation["overflow"] and observation["high_water_spool_bytes"] <= 0:
                        raise RuntimeError(f"{label}: no overflow")
                elif observation["lookup_hits"] != 5000 or observation["reverse_members"] != 65536:
                    raise RuntimeError(f"{label}: route membership mismatch")
            manifest["runs"].append({
                "label": label, "command": command, "exit_status": result.returncode,
                "observation": observation,
                "stdout_sha256": sha256(output / f"{label}.stdout"),
                "stderr_sha256": sha256(output / f"{label}.stderr"),
            })
            (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
            print(f"{label}: exit {result.returncode}", flush=True)
            if result.returncode:
                raise RuntimeError(f"{label} failed: {result.stderr}")
    (output / "completed").write_text("complete\n")


if __name__ == "__main__":
    main()
