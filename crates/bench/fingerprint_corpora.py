#!/usr/bin/env python3
"""Fingerprint benchmark sources and configuration outside timed runs."""

import argparse
import hashlib
import json
from pathlib import Path


CORPORA = (
    "musenalm", "lenz-archiv", "leipzig-multilingual",
    "amazon-reviews-2023", "nyc311",
)


def fingerprint(root: Path) -> dict:
    records = sorted((root / "data").rglob("*.json"))
    digest = hashlib.sha256()
    byte_count = 0
    for path in records:
        contents_hash = hashlib.sha256()
        with path.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                contents_hash.update(chunk)
                byte_count += len(chunk)
        stamp = path.stat()
        digest.update(json.dumps([
            path.relative_to(root).as_posix(), contents_hash.hexdigest(),
            stamp.st_size, stamp.st_mtime_ns, stamp.st_ctime_ns,
        ], separators=(",", ":")).encode())
        digest.update(b"\n")
    config = {}
    for path in sorted((root / ".objekte").rglob("*")):
        if path.is_file() and ".cache" not in path.parts:
            config[path.relative_to(root).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    manifest = root / "import-manifest.json"
    return {
        "record_count": len(records), "record_bytes": byte_count,
        "source_content_and_stat_sha256": digest.hexdigest(),
        "source_manifest_sha256": hashlib.sha256(manifest.read_bytes()).hexdigest()
        if manifest.is_file() else None,
        "configuration_sha256": config,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2] / "examples"
    result = {name: fingerprint(root / name) for name in CORPORA}
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
