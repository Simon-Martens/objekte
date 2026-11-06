#!/usr/bin/env python3
"""Save and hash complete FTS4 snapshots outside the timed build comparisons."""

import hashlib
import json
import os
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[5]
OUTPUT = Path(__file__).resolve().parent
SNAPSHOTS = ROOT / "target/recovery/signatures"
FIELDS = (
    "annotation=markdown", "preferred_title=plain",
    "title_statement=html", "contributors[].annotation=xml",
)


def digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def main() -> None:
    SNAPSHOTS.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["RAYON_NUM_THREADS"] = "16"
    results = {}
    for mode in "BD":
        for revision in ("reference", "candidate"):
            label = f"{revision}-{mode}"
            binary = ROOT / f"target/recovery/{label}/release/index-build-profile"
            snapshot = SNAPSHOTS / f"{label}.bigram"
            command = [
                "taskset", "-c", "0-15", str(binary),
                "--repo", str(ROOT / "examples/musenalm"),
                "--collection", "contents", "--encoding", "ascii",
                "--snapshot-path", str(snapshot),
            ]
            for field in FIELDS:
                command.extend(("--field", field))
            run = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True)
            (OUTPUT / f"signature-{label}.stdout.json").write_text(run.stdout)
            (OUTPUT / f"signature-{label}.stderr.log").write_text(run.stderr)
            (OUTPUT / f"signature-{label}.command.json").write_text(json.dumps({
                "command": command, "rayon_num_threads": 16,
                "exit_status": run.returncode, "binary_sha256": digest(binary),
            }, indent=2) + "\n")
            if run.returncode:
                raise RuntimeError(f"{label} failed: {run.stderr}")
            record = json.loads(run.stdout)
            if record["documents"] != 133948:
                raise RuntimeError(f"{label}: unexpected document count")
            results[label] = {
                "base_sha256": digest(snapshot),
                "secondary_sha256": digest(snapshot.with_suffix(".secondary")),
                "base_bytes": snapshot.stat().st_size,
                "secondary_bytes": snapshot.with_suffix(".secondary").stat().st_size,
                "index_sizes": record["index_sizes"],
                "compressed_index_bytes": record["compressed_index_bytes"],
            }
            print(label, results[label]["base_sha256"][:12],
                  results[label]["secondary_sha256"][:12], flush=True)
    (OUTPUT / "signatures.json").write_text(json.dumps(results, indent=2) + "\n")
    for mode in "BD":
        if results[f"reference-{mode}"] != results[f"candidate-{mode}"]:
            raise RuntimeError(f"{mode}: reference/candidate index signatures differ")


if __name__ == "__main__":
    main()
