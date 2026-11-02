#!/usr/bin/env python3
"""Create a repeatable repository for view and sort allocation measurements."""

import argparse
import json
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--records", type=int, default=20_000)
    args = parser.parse_args()
    if args.records < 1:
        parser.error("--records must be positive")
    data = args.root / "data" / "items"
    ui = args.root / ".editor" / "collections" / "items"
    for path in (data, ui):
        path.mkdir(parents=True, exist_ok=True)
    schema = {
        "type": "object",
        "properties": {
            "title": {"type": "string", "x-sort-index": True},
            "rank": {"type": "integer", "index": True},
            "category": {"type": "string", "index": True},
            "tags": {"type": "array", "items": {"type": "string"}, "x-sort-reduce": "min"},
            "payload": {"type": "object", "properties": {"notes": {"type": "string"}}},
        },
        "x-sort-indexes": [[
            {"field": "rank", "direction": "desc"},
            {"field": "title", "direction": "asc"},
        ]],
        "x-language": "de",
        "x-objekte": {
            "name": "items", "path": "data/items", "type": "data",
            "format": "json", "id": "filename",
        },
    }
    (ui / "schema.json").write_text(json.dumps(schema), encoding="utf-8")
    settings = {
        "display": "title",
        "list": {
            "columns": ["title", "rank", "category"],
            "filters": ["category", "rank"],
            "defaultSort": {"field": "rank", "direction": "asc"},
            "sortable": ["title", "rank", "category"],
        },
    }
    (ui / "ui.json").write_text(json.dumps(settings), encoding="utf-8")
    for index in range(args.records):
        record = {
            "title": f"{(index * 7919) % args.records:06} Äpfel {index % 17}",
            "rank": (index * 37) % 1000,
            "category": "all",
            "tags": ["Müller", "Straße", f"tag-{index % 23}"],
            "payload": {"notes": "Nested content. " * 96},
        }
        (data / f"{index:06}.json").write_text(
            json.dumps(record, ensure_ascii=False, separators=(",", ":")), encoding="utf-8"
        )


if __name__ == "__main__":
    main()
