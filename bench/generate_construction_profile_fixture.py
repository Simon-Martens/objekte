#!/usr/bin/env python3
"""Build deterministic JSONL or configured-subset construction fixtures."""

import argparse
import hashlib
import json
import shutil
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, default=Path("examples/musenalm/data/contents"))
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--records", type=int, default=16384)
    parser.add_argument("--configured-subset", action="store_true")
    parser.add_argument("--musenalm-root", type=Path, default=Path("examples/musenalm"))
    parser.add_argument("--per-collection", type=int, default=1024)
    args = parser.parse_args()
    if args.configured_subset:
        make_configured_subset(args)
        return
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


def make_configured_subset(args: argparse.Namespace) -> None:
    source = args.musenalm_root
    schema_source = source / ".objekte/collections"
    schema_dest = args.out / ".objekte/collections"
    schema_dest.mkdir(parents=True, exist_ok=False)
    digest = hashlib.sha256()
    copied = {}
    for schema in sorted(schema_source.glob("*.json")):
        raw = schema.read_bytes()
        digest.update(schema.name.encode())
        digest.update(raw)
        shutil.copy2(schema, schema_dest / schema.name)
        metadata = json.loads(raw)["x-objekte"]
        origin = source / metadata["path"]
        target = args.out / metadata["path"]
        target.mkdir(parents=True, exist_ok=False)
        paths = sorted(origin.glob("*.json"))[: args.per_collection]
        for path in paths:
            raw = path.read_bytes()
            digest.update(path.name.encode())
            digest.update(raw)
            shutil.copy2(path, target / path.name)
        copied[metadata["name"]] = len(paths)
    (args.out / "manifest.json").write_text(json.dumps({
        "source": str(source.resolve()),
        "per_collection_limit": args.per_collection,
        "copied_records": copied,
        "source_digest": digest.hexdigest(),
    }, indent=2) + "\n")
    print(args.out, sum(copied.values()), "records")


if __name__ == "__main__":
    main()
