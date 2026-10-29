#!/usr/bin/env python3
"""Apply the repository's blank-line convention after rustfmt."""

import argparse
from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]

METHOD = re.compile(
    r"(?m)^(?P<indent> {4,})}\n(?:\n)*(?=(?P=indent)(?:"
    r"(?:(?:pub(?:\([^)]*\))?|async|unsafe|const)\s+)*fn\b|///|#\[))"
)
TOP_LEVEL_FUNCTION = re.compile(
    r"(?m)^(})\n(?:\n)*(?=(?:(?:pub(?:\([^)]*\))?|async|unsafe|const)\s+)*fn\b|///|#\[)"
)
LARGE_ITEM = re.compile(
    r"^(?P<indent> *)(?:"
    r"(?:(?:pub(?:\([^)]*\))?|unsafe)\s+)*(?:impl|struct|enum|trait|union)\b"
    r"|macro_rules!\b"
    r"|(?:(?:pub(?:\([^)]*\))?)\s+)?mod\s+\w+\s*\{)"
)


def spaced(source: str) -> str:
    source = METHOD.sub(r"\g<indent>}\n\n", source)
    source = TOP_LEVEL_FUNCTION.sub(r"\1\n\n", source)
    lines = source.splitlines()
    items = []

    for index, line in enumerate(lines):
        match = LARGE_ITEM.match(line)
        if not match:
            continue

        indent = match.group("indent")
        start = index
        while start > 0 and (
            lines[start - 1].startswith(indent + "#[")
            or lines[start - 1].startswith(indent + "///")
            or lines[start - 1].startswith(indent + "// ")
        ):
            start -= 1

        if line.rstrip().endswith(";") or (
            "{" in line and line.count("{") == line.count("}")
        ):
            end = index
        else:
            end = next(
                (
                    position
                    for position in range(index + 1, len(lines))
                    if lines[position] == indent + "}"
                ),
                None,
            )
            if end is None:
                raise ValueError(f"Could not find the end of {line!r}")

        items.append((start, end))

    # Work backward so edits after an item cannot move the indices of earlier items.
    for start, end in reversed(items):
        indent = len(lines[start]) - len(lines[start].lstrip())
        next_line = end + 1
        while next_line < len(lines) and not lines[next_line]:
            next_line += 1
        closes_parent = (
            next_line < len(lines)
            and lines[next_line].strip() in ("}", "};")
            and len(lines[next_line]) - len(lines[next_line].lstrip()) <= indent
        )
        if next_line < len(lines):
            lines[end + 1 : next_line] = [] if closes_parent else ["", ""]

        previous_line = start - 1
        while previous_line >= 0 and not lines[previous_line]:
            previous_line -= 1
        opens_parent = (
            previous_line >= 0
            and lines[previous_line].rstrip().endswith("{")
            and len(lines[previous_line]) - len(lines[previous_line].lstrip()) <= indent
        )
        if previous_line >= 0:
            lines[previous_line + 1 : start] = [] if opens_parent else ["", ""]

    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()

    command = ["cargo", "fmt", "--all"]
    if args.check:
        command.append("--check")
    command += ["--", "--config", "blank_lines_upper_bound=2"]
    if subprocess.run(command, cwd=ROOT, check=False).returncode:
        return 1

    changed = []
    for path in sorted((ROOT / "crates").rglob("*.rs")):
        if "target" in path.parts:
            continue
        original = path.read_text()
        formatted = spaced(original)
        if original != formatted:
            changed.append(path.relative_to(ROOT))
            if not args.check:
                path.write_text(formatted)

    for path in changed:
        print(path)
    if args.check and changed:
        print("Rust blank-line convention is not satisfied", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
