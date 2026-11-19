#!/usr/bin/env python3
"""Classify changed Rust lines as production or test-only.

Modules are followed from lib.rs. Modules reachable only through test
configuration and physical lines in inline test items are test-only. Changed
lines are classified on the side of the diff where they exist.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path


def git_bytes(*args: str) -> bytes:
    return subprocess.check_output(["git", *args], stderr=subprocess.PIPE)


def git_text(*args: str) -> str:
    return git_bytes(*args).decode()


def exists(revision: str, path: str) -> bool:
    return (
        subprocess.run(
            ["git", "cat-file", "-e", f"{revision}:{path}"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        ).returncode
        == 0
    )


def physical_lines(data: bytes) -> int:
    return data.count(b"\n") + (0 if not data or data.endswith(b"\n") else 1)


def cfg_has_test(text: str) -> bool:
    if not text.startswith("#[cfg("):
        return False
    compact = re.sub(r"\s+", "", text)
    return "test" in compact and "not(test)" not in compact


def item_end(lines: list[str], start: int) -> int:
    """Return the zero-based final line of the item following one cfg attr."""
    i = start
    while i < len(lines):
        stripped = lines[i].strip()
        if not stripped or stripped.startswith(("#[", "///", "//!")):
            i += 1
            continue
        break
    if i >= len(lines):
        return len(lines) - 1

    first = lines[i].strip()
    keyword = re.match(
        r"(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?"
        r"(fn|impl|mod|struct|enum|union|trait|const|static|type|use|"
        r"macro_rules!|thread_local!|for|while|loop|match|if|else|let)\b",
        first,
    )
    block_expected = bool(
        keyword
        and keyword.group(1)
        in {
            "fn",
            "impl",
            "mod",
            "struct",
            "enum",
            "union",
            "trait",
            "macro_rules!",
            "thread_local!",
            "for",
            "while",
            "loop",
            "match",
            "if",
            "else",
        }
    )
    if not keyword and not any(token in first for token in ("{", ";")):
        # A cfg attribute on a field, parameter, or array element ends at its
        # comma rather than swallowing the enclosing function.
        for end in range(i, len(lines)):
            stripped = lines[end].strip()
            if ";" in stripped or stripped.endswith(","):
                return end
        return len(lines) - 1

    text = "\n".join(lines[i:])
    pos = 0
    state = "code"
    raw_hashes = 0
    depth = 0
    saw_brace = False
    while pos < len(text):
        char = text[pos]
        next_char = text[pos + 1] if pos + 1 < len(text) else "\0"
        if state == "line":
            if char == "\n":
                state = "code"
        elif state == "block":
            if char == "*" and next_char == "/":
                state = "code"
                pos += 1
        elif state == "string":
            if char == "\\":
                pos += 1
            elif char == '"':
                state = "code"
        elif state == "char":
            if char == "\\":
                pos += 1
            elif char == "'":
                state = "code"
        elif state == "raw":
            closing = '"' + "#" * raw_hashes
            if char == '"' and text.startswith(closing, pos):
                pos += raw_hashes + 1
                state = "code"
        else:
            if char == "/" and next_char == "/":
                state = "line"
                pos += 1
            elif char == "/" and next_char == "*":
                state = "block"
                pos += 1
            elif char == '"':
                state = "string"
            elif char == "'":
                # Ignore lifetimes; recognize only short char literals.
                short = (
                    pos + 2 < len(text)
                    and text[pos + 2] == "'"
                    and text[pos + 1] != "\\"
                )
                escaped = (
                    pos + 3 < len(text)
                    and text[pos + 1] == "\\"
                    and text[pos + 3] == "'"
                )
                if short or escaped:
                    state = "char"
            elif char == "r" and next_char in ('"', "#"):
                cursor = pos + 1
                hashes = 0
                while cursor < len(text) and text[cursor] == "#":
                    hashes += 1
                    cursor += 1
                if cursor < len(text) and text[cursor] == '"':
                    state = "raw"
                    raw_hashes = hashes
                    pos = cursor
            elif char == "{":
                saw_brace = True
                depth += 1
            elif char == "}":
                depth -= 1
                if saw_brace and depth == 0:
                    return i + text[: pos + 1].count("\n")
            elif char == ";" and not saw_brace and not block_expected:
                return i + text[: pos + 1].count("\n")
        pos += 1
    return len(lines) - 1


@dataclass
class SourceIndex:
    revision: str
    root: str
    files: set[str]
    lines: dict[str, list[str]]
    production_files: set[str]
    test_files: set[str]
    test_ranges: dict[str, list[tuple[int, int]]]

    def production_lines(self) -> int:
        return sum(
            len(self.lines[path])
            for path in self.production_files
        ) - sum(
            end - start + 1
            for path in self.production_files
            for start, end in self.test_ranges[path]
        )

    def test_lines(self) -> int:
        return sum(len(self.lines[path]) for path in self.files) - self.production_lines()

    def is_test_line(self, path: str, line: int) -> bool:
        return path in self.test_files or any(
            start <= line <= end for start, end in self.test_ranges.get(path, [])
        )


def module_target(path: str, name: str, path_attr: str | None, files: set[str]) -> str | None:
    if path_attr:
        return str(Path(path).parent / path_attr)
    base = Path(path).parent
    candidate = (
        base / name
        if Path(path).name in {"lib.rs", "mod.rs"}
        else base / Path(path).stem / name
    )
    file_candidate = str(candidate.with_suffix(".rs"))
    module_candidate = str(candidate / "mod.rs")
    if file_candidate in files:
        return file_candidate
    if module_candidate in files:
        return module_candidate
    return None


def source_index(revision: str, root: str) -> SourceIndex:
    files = {
        path
        for path in git_text("ls-tree", "-r", "--name-only", revision, root).splitlines()
        if path.endswith(".rs")
    }
    lines = {
        path: git_text("show", f"{revision}:{path}").splitlines() for path in files
    }
    entry = f"{root}/lib.rs"
    pending: list[tuple[str, bool]] = [(entry, False)]
    production: set[str] = set()
    test_files: set[str] = set()
    while pending:
        path, inherited_test = pending.pop()
        if path not in files:
            continue
        if inherited_test:
            test_files.add(path)
        else:
            production.add(path)
        cfg_test = False
        attrs: list[str] = []
        for line in lines[path]:
            stripped = line.strip()
            if cfg_has_test(stripped):
                cfg_test = True
                attrs.append(stripped)
                continue
            if stripped.startswith("#["):
                attrs.append(stripped)
                continue
            match = re.match(
                r"(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*([;{])", stripped
            )
            if match:
                name, kind = match.groups()
                path_attr = next(
                    (
                        match.group(1)
                        for attr in attrs
                        if (match := re.search(r'path\s*=\s*"([^"]+)"', attr))
                    ),
                    None,
                )
                if kind == ";":
                    target = module_target(path, name, path_attr, files)
                    if target is not None:
                        pending.append((target, inherited_test or cfg_test))
            if match or stripped:
                cfg_test = False
                attrs = []
    production -= test_files
    ranges: dict[str, list[tuple[int, int]]] = {}
    for path in production:
        found: list[tuple[int, int]] = []
        line = 0
        while line < len(lines[path]):
            if cfg_has_test(lines[path][line].strip()):
                end = item_end(lines[path], line + 1)
                found.append((line + 1, end + 1))
                line = end + 1
            else:
                line += 1
        ranges[path] = found
    return SourceIndex(
        revision,
        root,
        files,
        lines,
        production,
        test_files,
        ranges,
    )


def endpoint_for(account: dict, revision: str) -> dict[str, int] | None:
    for endpoint in account.get("original_merge", {}).values():
        if endpoint["revision"] == revision:
            return endpoint
    for endpoint in account.get("historical_endpoints", []):
        if endpoint["revision"] == revision:
            return endpoint
    return None


def diff_counts(
    from_revision: str,
    to_revision: str,
    old: SourceIndex,
    new: SourceIndex,
) -> dict[str, int]:
    diff = git_text(
        "diff",
        "--unified=0",
        "--find-renames",
        "--no-color",
        from_revision,
        to_revision,
        "--",
        old.root,
    ).splitlines()
    counts = {
        "production_added": 0,
        "production_removed": 0,
        "test_added": 0,
        "test_removed": 0,
    }
    old_path: str | None = None
    new_path: str | None = None
    old_line = new_line = 0
    for line in diff:
        if line.startswith("diff --git "):
            old_path = new_path = None
            continue
        if line.startswith("rename from "):
            old_path = f"{old.root}/{line.removeprefix('rename from ')}"
            continue
        if line.startswith("rename to "):
            new_path = f"{new.root}/{line.removeprefix('rename to ')}"
            continue
        if line.startswith("--- "):
            if line == "--- /dev/null":
                old_path = None
            else:
                old_path = line.removeprefix("--- a/")
            continue
        if line.startswith("+++ "):
            if line == "+++ /dev/null":
                new_path = None
            else:
                new_path = line.removeprefix("+++ b/")
            continue
        if line.startswith("@@"):
            match = re.match(r"@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@", line)
            if not match:
                raise RuntimeError(f"cannot parse hunk header: {line}")
            old_line = int(match.group(1))
            new_line = int(match.group(2))
            continue
        if line.startswith("-") and not line.startswith("---"):
            test = old_path is not None and old.is_test_line(old_path, old_line)
            counts["test_removed" if test else "production_removed"] += 1
            old_line += 1
        elif line.startswith("+") and not line.startswith("+++"):
            test = new_path is not None and new.is_test_line(new_path, new_line)
            counts["test_added" if test else "production_added"] += 1
            new_line += 1
        elif line.startswith(" "):
            old_line += 1
            new_line += 1
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
    old = source_index(args.from_revision, args.root)
    new = source_index(args.to_revision, args.root)
    physical_old = sum(len(lines) for lines in old.lines.values())
    physical_new = sum(len(lines) for lines in new.lines.values())
    old_expected = endpoint_for(account, args.from_revision)
    new_expected = endpoint_for(account, args.to_revision)
    if old_expected is not None and physical_old != old_expected["physical_lines"]:
        raise SystemExit(
            f"{args.from_revision}: physical lines {physical_old} != recorded "
            f"{old_expected['physical_lines']}"
        )
    if new_expected is not None and physical_new != new_expected["physical_lines"]:
        raise SystemExit(
            f"{args.to_revision}: physical lines {physical_new} != recorded "
            f"{new_expected['physical_lines']}"
        )

    result = diff_counts(args.from_revision, args.to_revision, old, new)
    result["production_net"] = result["production_added"] - result["production_removed"]
    result["test_net"] = result["test_added"] - result["test_removed"]
    for expected in account.get("historical_ranges", []):
        if (
            expected["from"] == args.from_revision
            and expected["to"] == args.to_revision
        ):
            for field in (
                "production_added",
                "production_removed",
                "test_added",
                "test_removed",
            ):
                if result[field] != expected[field]:
                    raise SystemExit(
                        f"{args.from_revision}..{args.to_revision}: {field} "
                        f"{result[field]} != recorded {expected[field]}"
                    )
            break
    if new_expected is not None:
        result["ending_production_lines"] = new_expected["production_lines"]
        result["ending_test_lines"] = new_expected["test_lines"]
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
