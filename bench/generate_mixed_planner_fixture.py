#!/usr/bin/env python3
"""Write the fixed selective-posting fixture used by bench_mixed_planner."""

import argparse
import hashlib
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--healthy", type=int, default=10000)
    args = parser.parse_args()
    if args.healthy < 2:
        parser.error("at least two healthy nonmatches are required")
    path = args.root / "data" / "records.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("wb") as output:
        output.write(b'{"title":"other","distractor":"needle"}\n')
        for _ in range(args.healthy - 1):
            output.write(b'{"title":"other"}\n')
        output.write(b'{"title":"needle"}\n')
        output.write(b'{"title":needle\n')
    print(f"{path} sha256={hashlib.sha256(path.read_bytes()).hexdigest()}")


if __name__ == "__main__":
    main()
