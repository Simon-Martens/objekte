#!/usr/bin/env python3
"""Create the stable, one-record-per-file probe fixture outside a source tree."""

import argparse
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    args = parser.parse_args()
    root = args.root
    if root.exists():
        raise SystemExit(f"fixture already exists: {root}")
    data = root / "data"
    data.mkdir(parents=True)
    for number in range(16_384):
        note = "Leipzig" if number % 2 == 0 else "Wien"
        (data / f"r{number:05}.json").write_text(
            f'{{"year":{1800 + number % 16},"note":"{note}"}}\n'
        )


if __name__ == "__main__":
    main()
