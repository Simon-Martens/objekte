#!/usr/bin/env python3
"""Request clean Musenalm file pages be dropped before a cold-read experiment.

This is an advisory data-page eviction, not a machine-wide cache reset. The
caller should record the output and avoid other readers until its run starts.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path


def evict(root: Path) -> dict[str, int]:
    pending = [root]
    files = bytes_total = failed = 0
    while pending:
        directory = pending.pop()
        with os.scandir(directory) as entries:
            for entry in entries:
                if entry.is_dir(follow_symlinks=False):
                    pending.append(Path(entry.path))
                elif entry.is_file(follow_symlinks=False):
                    try:
                        descriptor = os.open(entry.path, os.O_RDONLY)
                        try:
                            size = os.fstat(descriptor).st_size
                            os.posix_fadvise(descriptor, 0, 0, os.POSIX_FADV_DONTNEED)
                        finally:
                            os.close(descriptor)
                        files += 1
                        bytes_total += size
                    except OSError:
                        failed += 1
    return {"files": files, "bytes": bytes_total, "failures": failed}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    args = parser.parse_args()
    result = evict(args.root.resolve())
    print(json.dumps({"method": "POSIX_FADV_DONTNEED", **result}, sort_keys=True))
    if result["failures"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
