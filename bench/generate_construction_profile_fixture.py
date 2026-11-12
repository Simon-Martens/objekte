#!/usr/bin/env python3
"""Pack deterministic Musenalm JSON records into one large JSONL source file."""

import argparse
import hashlib
import json
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, default=Path("examples/musenalm/data/contents"))
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--records", type=int, default=16384)
    args = parser.parse_args()
    paths = sorted(args.source.glob("*.json"))[: args.records]
    if len(paths) != args.records:
        raise SystemExit(f"requested {args.records} records, found {len(paths)}")
    data = args.out / "data"
    data.mkdir(parents=True, exist_ok=False)
    target = data / "records.jsonl"
    source_digest = hashlib.sha256()
    output_digest = hashlib.sha256()
    with target.open("wb") as destination:
        for path in paths:
            raw = path.read_bytes()
            source_digest.update(path.name.encode())
            source_digest.update(raw)
            line = json.dumps(json.loads(raw), ensure_ascii=False, separators=(",", ":")).encode() + b"\n"
            destination.write(line)
            output_digest.update(line)
    (args.out / "manifest.json").write_text(json.dumps({
        "source": str(args.source.resolve()),
        "records": len(paths),
        "source_digest": source_digest.hexdigest(),
        "jsonl_digest": output_digest.hexdigest(),
        "jsonl_bytes": target.stat().st_size,
    }, indent=2) + "\n")
    print(target, target.stat().st_size)


if __name__ == "__main__":
    main()
