#!/usr/bin/env python3
"""Generate the deterministic synthetic fixture required by the stable-index task.

The generator is intentionally outside the measured backend work. It creates
100 files with 1,000 JSON Lines records each (100,000 records), 100,000
distinct high-cardinality scalar keys and one million tag memberships.
"""

import argparse
import json
import pathlib
import shutil


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=pathlib.Path)
    parser.add_argument("--records", type=int, default=100_000)
    parser.add_argument("--records-per-file", type=int, default=1_000)
    args = parser.parse_args()

    data = args.root / "data"
    if args.root.exists():
        shutil.rmtree(args.root)
    data.mkdir(parents=True)

    common = "the quick brown fox jumps over the lazy dog"
    records = args.records
    per_file = args.records_per_file
    files = (records + per_file - 1) // per_file
    for file_index in range(files):
        start = file_index * per_file
        end = min(start + per_file, records)
        path = data / f"part-{file_index:04d}.jsonl"
        with path.open("w", encoding="utf-8") as handle:
            for index in range(start, end):
                tags = [f"tag{(index * 7 + offset) % 10_000:05d}" for offset in range(10)]
                record = {
                    "id": f"r{index:06d}",
                    "high_card": f"k{index:06d}",
                    "bucket": index % 10_000,
                    "flag": (index % 3) == 0,
                    "text": f"{common} synthetic item {index % 1_000:03d}",
                    "tags": tags,
                    "nested": {
                        "group": f"g{index % 100:02d}",
                        "value": index % 1_000,
                    },
                }
                handle.write(json.dumps(record, separators=(",", ":")))
                handle.write("\n")
    print(
        json.dumps(
            {
                "root": str(args.root),
                "records": records,
                "files": files,
                "distinct_high_card_keys": records,
                "tag_memberships": records * 10,
            }
        )
    )


if __name__ == "__main__":
    main()
