"""Generate deterministic JSON records for candidate-verification benchmarks."""

import argparse
import hashlib
import json
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--records", type=int, default=2048)
    parser.add_argument("--length", type=int, default=8192)
    parser.add_argument("--escaped", action="store_true")
    parser.add_argument("--duplicates", type=int, default=0)
    parser.add_argument("--depth", type=int, default=0)
    args = parser.parse_args()
    if args.records < 1 or args.length < 1 or args.duplicates < 0 or args.depth < 0:
        parser.error("records and length must be positive; duplicates and depth nonnegative")
    if args.depth > 120:
        parser.error("depth must not exceed 120")

    data_dir = args.root / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    if next(data_dir.glob("*.json"), None) is not None:
        parser.error(f"{data_dir} already contains JSON files")

    alphabet = 'é"\\x' if args.escaped else "x"
    padding = (alphabet * ((args.length + len(alphabet) - 1) // len(alphabet)))[
        : args.length
    ]
    corpus_hash = hashlib.sha256()
    total_bytes = 0
    for number in range(1, args.records + 1):
        position = number % 3
        text = ("good" + padding) if position == 0 else (
            (padding + "good") if position == 1 else padding
        )
        record = {
            "rating": 5.0 if number % 3 == 0 else 4.0,
            "helpful_vote": number % 10,
            "title": f"Review {number}",
            "text": text,
            "verified_purchase": number % 2 == 0,
        }
        encoded = json.dumps(
            record, ensure_ascii=args.escaped, separators=(",", ":")
        ).encode("utf-8")
        if args.duplicates or args.depth:
            duplicate_members = ",".join(
                f'"dup":"value{member}"' for member in range(args.duplicates)
            )
            nested = "{" + duplicate_members + ("," if duplicate_members else "") + '"dup":{},"leaf":"good"}'
            for _ in range(args.depth):
                nested = '{"nest":' + nested + "}"
            encoded = encoded[:-1] + b',"nested":' + nested.encode("ascii") + b"}"
        name = f"{number:07d}.json"
        (data_dir / name).write_bytes(encoded)
        corpus_hash.update(len(name).to_bytes(8, "little"))
        corpus_hash.update(name.encode("ascii"))
        corpus_hash.update(len(encoded).to_bytes(8, "little"))
        corpus_hash.update(encoded)
        total_bytes += len(encoded)

    manifest = {
        "kind": "synthetic_candidate_verification",
        "records": args.records,
        "logical_text_length": args.length,
        "escaped": args.escaped,
        "duplicates_per_record": args.duplicates,
        "nested_depth": args.depth,
        "positions": "record_number % 3: 0 early, 1 late, 2 no match",
        "source_bytes": total_bytes,
        "corpus_sha256": corpus_hash.hexdigest(),
    }
    (args.root / "fixture-manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )
    print(json.dumps(manifest))


if __name__ == "__main__":
    main()
