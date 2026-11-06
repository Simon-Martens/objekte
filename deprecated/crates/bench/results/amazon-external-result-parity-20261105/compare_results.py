#!/usr/bin/env python3
"""Compare every jq and Objekte result ID for the Amazon review fixture."""

import hashlib
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[4]
RAW = ROOT / "target/benchmarks/amazon-external-result-parity-20261105"
OUTPUT = Path(__file__).resolve().parent / "result-parity.json"


def digest(ids: list[bytes]) -> str:
    return hashlib.sha256(b"\n".join(ids) + b"\n").hexdigest()


def read_ids(name: str) -> list[bytes]:
    ids = (RAW / name).read_bytes().splitlines()
    if len(ids) != len(set(ids)):
        raise ValueError(f"duplicate IDs in {name}")
    return ids


def main() -> None:
    report = {"records": 1_000_000, "comparisons": {}}
    expected_by_label = {}
    for label in ("positive", "negative"):
        expected = read_ids(f"jq-{label}.ids")
        expected_by_label[label] = set(expected)
        comparison = {
            "jq_count": len(expected),
            "canonical_sorted_sha256": digest(sorted(expected)),
            "backends": {},
        }
        for backend in ("bigram", "direct"):
            actual = read_ids(f"objekte-{backend}-{label}.ids")
            actual_set = set(actual)
            missing = sorted(expected_by_label[label] - actual_set)
            extra = sorted(actual_set - expected_by_label[label])
            summary = json.loads((RAW / f"objekte-{backend}-{label}.json").read_text())
            comparison["backends"][backend] = {
                "count": len(actual),
                "missing": len(missing),
                "extra": len(extra),
                "first_missing": [value.decode() for value in missing[:10]],
                "first_extra": [value.decode() for value in extra[:10]],
                "same_as_reverse_archive_order": actual == list(reversed(expected)),
                "canonical_sorted_sha256": digest(sorted(actual)),
                "source_reads": summary["source_reads"],
                "parses": summary["parses"],
                "scans": summary["scans"],
                "returned": summary["returned"],
                "total": summary["total"],
                "truncated": summary["truncated"],
                "predicate_cache_hits": summary["predicate_cache_hits"],
                "ordered_result_hits": summary["ordered_result_hits"],
            }
            if missing or extra or len(actual) != len(expected):
                raise AssertionError(f"{backend} {label} differs from jq")
            if summary["truncated"] or summary["total"] != len(expected):
                raise AssertionError(f"{backend} {label} returned an incomplete count")
        report["comparisons"][label] = comparison

    if expected_by_label["positive"] & expected_by_label["negative"]:
        raise AssertionError("jq positive and negative results overlap")
    if len(expected_by_label["positive"] | expected_by_label["negative"]) != 1_000_000:
        raise AssertionError("jq did not classify every input record")
    if read_ids("jq-nonstring.ids"):
        raise AssertionError("some review text fields are not strings")
    OUTPUT.write_text(json.dumps(report, indent=2) + "\n")
    print(f"All four Objekte result lists exactly match jq; summary: {OUTPUT}")


if __name__ == "__main__":
    main()
