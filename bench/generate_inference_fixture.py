#!/usr/bin/env python3
"""Generate a deterministic JSONL corpus for inference build measurements."""

import argparse
import hashlib
import json
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--records", type=int, default=100_000)
    args = parser.parse_args()
    if args.records <= 0:
        parser.error("--records must be positive")
    data = args.root / "data"
    data.mkdir(parents=True, exist_ok=True)
    output = data / "records.jsonl"
    digest = hashlib.sha256()
    with output.open("wb") as stream:
        for number in range(args.records):
            identifier = f"550e8400-e29b-41d4-a716-{number:012d}"
            kind = f"type-{number % 5}"
            record = {
                "uuid": identifier,
                "manual": identifier,
                "blocked": identifier,
                "kind": kind,
                "deep": {"outer": {"inner": {"flag": number % 2 == 0, "code": kind}}},
                "tags": [] if number % 11 == 0 else [kind, kind],
                "short_mixed": "x" * 65 if number % 1000 == 0 else f"short-{number:010d}",
                "body": f"Long free text for record {number:010d}: " + "q" * 96,
            }
            encoded = (json.dumps(record, ensure_ascii=False, separators=(",", ":")) + "\n").encode()
            stream.write(encoded)
            digest.update(encoded)
    manifest = {
        "records": args.records,
        "sha256": digest.hexdigest(),
        "bytes": output.stat().st_size,
        "source": str(output),
    }
    (args.root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest))


if __name__ == "__main__":
    main()
