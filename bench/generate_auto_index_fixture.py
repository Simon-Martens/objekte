#!/usr/bin/env python3
"""Create reproducible JSON folders for the short-string index measurements."""

import argparse
import hashlib
import json
from pathlib import Path


def uuid(slot: int) -> str:
    return f"550e8400-e29b-41d4-a716-{slot:012x}"


def document(slot: int, variant: str) -> dict:
    if variant == "uuid":
        return {"code": uuid(slot)}
    if variant == "paths":
        return {"common": "shared", **{f"field_{slot:04}_{part:03}": f"v{part}" for part in range(100)}}
    return {
        "code": uuid(slot),
        "year": 1750 + slot % 300,
        "group": f"g{slot % 8}",
        "active": slot % 2 == 0,
        "short63": f"{slot:06}" + "a" * 57,
        "short64": f"{slot:06}" + "b" * 58,
        "long65": f"{slot:06}" + "L" * 59,
        "mixed": [f"m{slot % 8}", "long" * 17] if slot % 32 == 0 else [f"m{slot % 8}"],
        "body": f"A shared phrase in long free text for document {slot}. " * 3,
        "nested": {"label": f"label-{slot % 16}"},
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--variant", choices=["medium", "uuid", "paths"], required=True)
    parser.add_argument("--records", type=int, required=True)
    args = parser.parse_args()
    if args.records < 1 or args.root.exists():
        parser.error("record count must be positive and root must not exist")
    args.root.mkdir(parents=True)
    digest = hashlib.sha256()
    for slot in range(args.records):
        name = f"{slot:06}.json"
        content = json.dumps(document(slot, args.variant), ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
        (args.root / name).write_bytes(content)
        digest.update(name.encode() + b"\0" + content + b"\0")
    manifest = {"variant": args.variant, "records": args.records, "content_sha256": digest.hexdigest()}
    args.root.with_suffix(".manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    if args.variant == "medium":
        schema = {
            "type": "object",
            "properties": {
                "code": {"type": "string", "index": True},
                "year": {"type": "integer", "index": True},
                "active": {"type": "boolean", "index": True},
                "group": {"type": "string"},
                "body": {"type": "string", "index": False},
                "mixed": {"type": "array", "items": {"type": "string"}},
            },
        }
        args.root.with_suffix(".schema.json").write_text(json.dumps(schema, indent=2) + "\n")
        queries = [
            f"code = '{uuid(36)}'",
            f"code >= '{uuid(100)}' && code < '{uuid(110)}'",
            "year >= 1800 && year < 1805",
            "group = 'g3'",
            "mixed ?= 'm3'",
            "long65 > 'L'",
            "body ~ 'shared phrase'",
            "year >= 1800 SORT BY year DESC, code ASC",
            '"shared phrase"',
        ]
        args.root.with_suffix(".queries.txt").write_text("\n".join(queries) + "\n")
    print(json.dumps({"root": str(args.root), **manifest}))


if __name__ == "__main__":
    main()
