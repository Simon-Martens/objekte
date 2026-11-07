#!/usr/bin/env python3
"""Compare obj FTS membership with an independent JSON scalar-value walk."""

import argparse
import json
import subprocess
from dataclasses import dataclass
from pathlib import Path


@dataclass
class NumberToken:
    text: str


@dataclass
class ObjectPairs:
    pairs: list


NEEDLES = (
    "Goethe",
    "first occurrence",
    "last occurrence",
    "Gründe",
    "true",
    "null",
    "123",
    "1e1000000",
    "a root string",
    "punctuation",
    "title",  # A property name alone must not satisfy whole-file FTS.
    "text123",  # Adjacent scalar values must not become one segment.
)


def scalar_texts(value):
    if isinstance(value, ObjectPairs):
        for _, member in value.pairs:
            yield from scalar_texts(member)
    elif isinstance(value, list):
        for member in value:
            yield from scalar_texts(member)
    elif isinstance(value, NumberToken):
        yield value.text
    elif isinstance(value, str):
        yield value
    elif value is True:
        yield "true"
    elif value is False:
        yield "false"
    elif value is None:
        yield "null"
    else:
        raise AssertionError(type(value))


def expected_membership(root: Path, needle: str) -> set[str]:
    result = set()
    for path in root.rglob("*.json"):
        relative = path.relative_to(root)
        if any(part in {".git", ".objekte"} for part in relative.parts[:-1]):
            continue
        try:
            value = json.loads(
                path.read_text(),
                parse_int=NumberToken,
                parse_float=NumberToken,
                object_pairs_hook=ObjectPairs,
            )
        except (UnicodeError, json.JSONDecodeError):
            # The fixture's malformed file follows the raw-fallback contract;
            # none of these selected needles occur in its raw bytes.
            assert needle not in path.read_bytes().decode("utf-8", "replace")
            continue
        if any(needle in scalar for scalar in scalar_texts(value)):
            result.add(relative.as_posix())
    return result


def observed_membership(binary: Path, root: Path, needle: str) -> set[str]:
    process = subprocess.run(
        [str(binary), "--path", str(root), "--fts", needle],
        capture_output=True,
        check=False,
    )
    if process.returncode:
        raise AssertionError((needle, process.returncode, process.stderr.decode()))
    events = [json.loads(line) for line in process.stdout.splitlines()]
    assert events[-1]["type"] == "run_complete"
    query = [event for event in events if event["type"] == "query_complete"]
    assert len(query) == 1 and query[0]["total_exact"] is True
    hits = [event for event in events if event["type"] == "hit"]
    assert query[0]["exact_total"] == len(hits)
    return {event["file"]["record_id"] for event in hits}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    args = parser.parse_args()
    failures = 0
    for needle in NEEDLES:
        expected = expected_membership(args.root, needle)
        observed = observed_membership(args.binary.resolve(), args.root.resolve(), needle)
        status = "PASS" if expected == observed else "FAIL"
        print(f"{status}\t{needle!r}\texpected={len(expected)}\tobserved={len(observed)}")
        if expected != observed:
            print(f"  missing={sorted(expected - observed)} extra={sorted(observed - expected)}")
            failures += 1
    if failures:
        raise SystemExit(f"{failures} independent scalar oracle mismatches")


if __name__ == "__main__":
    main()
