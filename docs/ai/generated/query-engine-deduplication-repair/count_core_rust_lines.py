#!/usr/bin/env python3
"""Classify changed Rust lines as production or test-only.

The original endpoint counts in source-size-account.json were independently
reviewed. This script verifies their physical-line totals and reproduces the
repair delta by classifying changed lines against the old and new trees.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path


def git(*args: str) -> bytes:
    return subprocess.check_output(["git", *args], text=False)


def physical_lines(revision: str, root: str) -> int:
    paths = git("ls-tree", "-r", "--name-only", revision, root).decode().splitlines()
    total = 0
    for path in paths:
        if not path.endswith(".rs"):
            continue
        data = git("show", f"{revision}:{path}")
        total += data.count(b"\n") + (0 if not data or data.endswith(b"\n") else 1)
    return total


def source(revision: str, path: str) -> list[str]:
    return git("show", f"{revision}:{path}").decode().splitlines()


def item_end(lines: list[str], start: int) -> int:
    i = start
    while i < len(lines):
        text = lines[i]
        stripped = text.strip()
        if not stripped or stripped.startswith("#["):
            i += 1
            continue
        if re.match(r"(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{", stripped):
            return len(lines) - 1
        if ";" in stripped.split("{", 1)[0] or "{" not in stripped:
            return i
        depth = 0
        while i < len(lines):
            depth += lines[i].count("{") - lines[i].count("}")
            if depth <= 0:
                return i
            i += 1
        return len(lines) - 1
    return len(lines) - 1


def test_ranges(revision: str, path: str) -> list[tuple[int, int]]:
    lines = source(revision, path)
    ranges = []
    i = 0
    while i < len(lines):
        if lines[i].strip().startswith("#[cfg(test)]"):
            end = item_end(lines, i + 1)
            ranges.append((i + 1, end + 1))
            i = end + 1
        else:
            i += 1
    return ranges


def test_file(path: str) -> bool:
    parts = Path(path).parts
    return "test" in Path(path).name or "tests" in parts or "test" in parts


def changed_lines(revision: str, root: str) -> list[str]:
    return git(
        "diff",
        "--unified=0",
        "--no-color",
        revision,
        "HEAD",
        "--",
        root,
    ).decode().splitlines()


def repair_counts(from_revision: str, to_revision: str, root: str) -> dict[str, int]:
    diff = git(
        "diff",
        "--unified=0",
        "--no-color",
        from_revision,
        to_revision,
        "--",
        root,
    ).decode().splitlines()
    counts = {
        "production_added": 0,
        "production_removed": 0,
        "test_added": 0,
        "test_removed": 0,
    }
    path = None
    old_line = 0
    new_line = 0
    old_ranges: list[tuple[int, int]] = []
    new_ranges: list[tuple[int, int]] = []
    in_ranges = lambda line, ranges: any(start <= line <= end for start, end in ranges)
    for line in diff:
        if line.startswith("+++ b/"):
            path = line[6:]
            old_ranges = test_ranges(from_revision, path)
            new_ranges = test_ranges(to_revision, path)
            continue
        if line.startswith("@@"):
            match = re.match(r"@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@", line)
            if not match:
                raise RuntimeError(f"cannot parse hunk header: {line}")
            old_line = int(match.group(1))
            new_line = int(match.group(2))
            continue
        if path is None or line.startswith(("--- ", "diff --git ")):
            continue
        if line.startswith("-") and not line.startswith("---"):
            is_test = test_file(path) or in_ranges(old_line, old_ranges)
            counts["test_removed" if is_test else "production_removed"] += 1
            old_line += 1
        elif line.startswith("+") and not line.startswith("+++"):
            is_test = test_file(path) or in_ranges(new_line, new_ranges)
            counts["test_added" if is_test else "production_added"] += 1
            new_line += 1
        elif line.startswith(" "):
            old_line += 1
            new_line += 1
    counts["production_net"] = counts["production_added"] - counts["production_removed"]
    counts["test_net"] = counts["test_added"] - counts["test_removed"]
    return counts


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--from", dest="from_revision", required=True)
    parser.add_argument("--to", dest="to_revision", default="HEAD")
    parser.add_argument("--root", default="crates/core/src")
    parser.add_argument(
        "--account",
        default="docs/ai/generated/query-engine-deduplication-repair/source-size-account.json",
    )
    args = parser.parse_args()
    account = json.loads(Path(args.account).read_text())
    for endpoint in account["original_merge"].values():
        actual = physical_lines(endpoint["revision"], args.root)
        expected = endpoint["production_lines"] + endpoint["test_lines"]
        if actual != expected:
            raise SystemExit(
                f"{endpoint['revision']}: physical lines {actual} != recorded {expected}"
            )
    result = repair_counts(args.from_revision, args.to_revision, args.root)
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
