#!/usr/bin/env python3
"""Snapshot the ordered Musenalm path, modification-time and size inventory."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    root = args.root.resolve(strict=True)
    rows: list[tuple[str, int, int]] = []
    total_bytes = 0
    for directory, directory_names, file_names in os.walk(root):
        directory_names.sort()
        file_names.sort()
        for file_name in file_names:
            path = Path(directory, file_name)
            metadata = path.stat()
            relative = path.relative_to(root).as_posix()
            rows.append((relative, metadata.st_mtime_ns, metadata.st_size))
            total_bytes += metadata.st_size

    rows.sort()
    digest = hashlib.sha256()
    for relative, modified_ns, size in rows:
        digest.update(relative.encode("utf-8"))
        digest.update(modified_ns.to_bytes(16, "little", signed=True))
        digest.update(size.to_bytes(8, "little", signed=False))

    result = {
        "root": str(root),
        "file_count": len(rows),
        "total_bytes": total_bytes,
        "inventory_sha256": digest.hexdigest(),
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
