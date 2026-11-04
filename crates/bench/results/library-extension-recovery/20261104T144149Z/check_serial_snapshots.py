#!/usr/bin/env python3
"""Check whether one-worker snapshot bytes are deterministic across builds."""

import hashlib
import json
import os
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[5]
OUTPUT = Path(__file__).resolve().parent
SNAPSHOTS = ROOT / "target/recovery/signatures"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main() -> None:
    env = os.environ.copy()
    env["RAYON_NUM_THREADS"] = "1"
    results = {}
    for label, binary_label in (
        ("reference-1", "reference-B"),
        ("candidate-1", "candidate-B"),
        ("reference-2", "reference-B"),
    ):
        binary = ROOT / f"target/recovery/{binary_label}/release/index-build-profile"
        snapshot = SNAPSHOTS / f"serial-{label}.bigram"
        command = [
            "taskset", "-c", "0", str(binary),
            "--repo", str(ROOT / "examples/musenalm"),
            "--collection", "contents", "--encoding", "ascii",
            "--snapshot-path", str(snapshot),
            "--field", "annotation=markdown",
            "--field", "preferred_title=plain",
            "--field", "title_statement=html",
            "--field", "contributors[].annotation=xml",
        ]
        run = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True)
        (OUTPUT / f"serial-{label}.stdout.json").write_text(run.stdout)
        (OUTPUT / f"serial-{label}.stderr.log").write_text(run.stderr)
        (OUTPUT / f"serial-{label}.command.json").write_text(json.dumps({
            "command": command, "rayon_num_threads": 1,
            "exit_status": run.returncode, "binary_sha256": sha256(binary),
        }, indent=2) + "\n")
        if run.returncode:
            raise RuntimeError(f"{label} failed: {run.stderr}")
        record = json.loads(run.stdout)
        results[label] = {
            "base_sha256": sha256(snapshot),
            "secondary_sha256": sha256(snapshot.with_suffix(".secondary")),
            "base_bytes": snapshot.stat().st_size,
            "secondary_bytes": snapshot.with_suffix(".secondary").stat().st_size,
            "compressed_index_bytes": record["compressed_index_bytes"],
            "index_sizes": record["index_sizes"],
            "documents": record["documents"],
        }
        print(label, results[label]["base_sha256"][:12],
              results[label]["secondary_sha256"][:12], flush=True)
    (OUTPUT / "serial-signatures.json").write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
