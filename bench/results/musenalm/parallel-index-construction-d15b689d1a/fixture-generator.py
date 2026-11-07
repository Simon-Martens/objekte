#!/usr/bin/env python3
"""Generate deterministic one-file and mixed-size build fixtures."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


SCHEMA = {
    "type": "object",
    "properties": {
        "title": {"type": "string", "index": True},
        "year": {"type": "integer", "index": True},
        "tags": {"type": "array", "items": {"type": "string"}},
        "note": {"type": "string"},
    },
    "x-objekte": {
        "name": "fixture",
        "path": "data/fixture",
        "type": "data",
        "format": "jsonl",
        "id": "generated",
    },
}


def line(index: int) -> bytes:
    return (
        json.dumps(
            {
                "title": f"Record {index:06d} Müller Straße",
                "year": 1800 + index % 200,
                "tags": ["alpha", "beta"],
                "note": "x" * 32,
            },
            ensure_ascii=False,
            separators=(",", ":"),
        ).encode()
        + b"\n"
    )


def generate(root: Path, layout: str) -> dict:
    schema_dir = root / ".objekte/collections"
    data_dir = root / "data/fixture"
    schema_dir.mkdir(parents=True, exist_ok=True)
    data_dir.mkdir(parents=True, exist_ok=True)
    schema_path = schema_dir / "fixture.schema.json"
    schema_path.write_text(json.dumps(SCHEMA, sort_keys=True, indent=2) + "\n")
    files = []
    large_count = 100_000 if layout == "large" else 80_000
    large = data_dir / "0000-large.jsonl"
    with large.open("wb") as output:
        for index in range(large_count):
            output.write(line(index))
    files.append(large)
    count = large_count
    if layout == "mixed":
        for file_number in range(512):
            path = data_dir / f"{file_number + 1:04d}-small.jsonl"
            with path.open("wb") as output:
                for record_number in range(20):
                    output.write(line(count + record_number))
            count += 20
            files.append(path)
    return {
        "layout": layout,
        "records": count,
        "files": len(files),
        "schema_sha256": hashlib.sha256(schema_path.read_bytes()).hexdigest(),
        "source_sha256": {
            str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in files
        },
        "source_bytes": sum(path.stat().st_size for path in files),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--layout", choices=("large", "mixed"), required=True)
    args = parser.parse_args()
    print(json.dumps(generate(args.root, args.layout), sort_keys=True))
