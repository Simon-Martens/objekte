#!/usr/bin/env python3
"""Compare complete new-core ID sets with handwritten jq over raw JSON files."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import subprocess
import sys
import tempfile
from collections import defaultdict
from pathlib import Path


CASES = tuple(f"Q{number:03}" for number in range(1, 151))
ROOT = Path(__file__).resolve().parent.parent
PROGRAM = ROOT / "bench/oracle_musenalm.jq"


def command(*args: str, cwd: Path = ROOT) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, cwd=cwd, text=True, capture_output=True, check=True)


def save_membership(path: Path, value: dict[str, list[list[str]]]) -> None:
    encoded = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
    path.write_bytes(gzip.compress(encoded, compresslevel=9, mtime=0))


def membership_digests(value: dict[str, list[list[str]]]) -> dict[str, str]:
    return {
        case: hashlib.sha256(json.dumps(ids, ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()
        for case, ids in value.items()
    }


def definitions(fixture: Path) -> list[tuple[str, Path]]:
    found = []
    for schema in sorted((fixture / ".objekte/collections").glob("*.schema.json")):
        declaration = json.loads(schema.read_text())["x-objekte"]
        if declaration["format"] != "json" or declaration["id"] != "filename":
            raise ValueError(f"unsupported oracle source declaration: {schema}")
        found.append((declaration["name"], fixture / declaration["path"]))
    if len(found) != 7:
        raise ValueError(f"expected seven collections, found {len(found)}")
    return found


def birth_times(paths: list[Path]) -> dict[str, int | None]:
    values = command("stat", "-c", "%W", *(str(path) for path in paths)).stdout.splitlines()
    if len(values) != len(paths):
        raise ValueError("stat did not report every source birth time")
    return {
        str(path): (int(value) if int(value) > 0 else None)
        for path, value in zip(paths, values)
    }


def raw_oracle(fixture: Path) -> tuple[dict[str, list[list[str]]], int, str]:
    matches: dict[str, set[tuple[str, str]]] = defaultdict(set)
    fingerprint = hashlib.sha256()
    files_seen = 0
    with tempfile.TemporaryDirectory(prefix="objekte-jq-") as scratch:
        metadata_file = Path(scratch) / "metadata.json"
        for collection, directory in definitions(fixture):
            paths = sorted(directory.glob("*.json"))
            print(f"jq scanning {collection}: {len(paths)} source files", file=sys.stderr, flush=True)
            for path in paths:
                info = path.stat()
                fingerprint.update(str(path.relative_to(fixture)).encode())
                fingerprint.update(info.st_size.to_bytes(8, "little"))
                fingerprint.update(info.st_mtime_ns.to_bytes(16, "little", signed=True))
            for start in range(0, len(paths), 1024):
                batch = paths[start : start + 1024]
                births = birth_times(batch) if collection == "items" else {}
                metadata = {}
                for path in batch:
                    info = path.stat()
                    metadata[str(path)] = {
                        "size": info.st_size,
                        "modified_seconds": info.st_mtime_ns // 1_000_000_000,
                        "birth_seconds": births.get(str(path)),
                        "relative_path": str(path.relative_to(fixture)),
                    }
                metadata_file.write_text(json.dumps(metadata, separators=(",", ":")))
                output = command(
                    "jq", "-cn", "--arg", "collection", collection,
                    "--slurpfile", "metadata", str(metadata_file),
                    "-f", str(PROGRAM), *(str(path) for path in batch)
                ).stdout
                for line in output.splitlines():
                    value = json.loads(line)
                    key = (value["collection"], value["id"])
                    case = value["case"]
                    if case not in CASES or key in matches[case]:
                        raise ValueError(f"duplicate or unknown oracle match: {value}")
                    matches[case].add(key)
            files_seen += len(paths)
    return (
        {case: [list(pair) for pair in sorted(matches[case])] for case in CASES},
        files_seen,
        fingerprint.hexdigest(),
    )


def core_ids(fixture: Path, alphabet: str) -> dict[str, list[list[str]]]:
    output = command(
        "cargo", "run", "--release", "--example", "dump_musenalm_ids", "--",
        "--root", str(fixture), "--cases", ",".join(CASES),
        "--alphabet", alphabet,
    ).stdout
    actual = {}
    for line in output.splitlines():
        value = json.loads(line)
        case = value["id"]
        if case in actual or case not in CASES or value["count"] != len(value["ids"]):
            raise ValueError(f"invalid new-core membership row: {case}")
        actual[case] = value["ids"]
    if set(actual) != set(CASES):
        raise ValueError(f"missing new-core cases: {set(CASES) - set(actual)}")
    return actual


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT / "examples/musenalm")
    parser.add_argument("--result-dir", type=Path, required=True)
    parser.add_argument("--save-membership", action="store_true", help="also save all complete ID sets as gzip JSON")
    args = parser.parse_args()
    fixture = args.root.resolve()
    result_dir = args.result_dir.resolve()
    result_dir.mkdir(parents=True, exist_ok=True)
    expected, files_seen, fingerprint = raw_oracle(fixture)
    if args.save_membership:
        save_membership(result_dir / "jq-expected.json.gz", expected)
    digests = {"jq": membership_digests(expected)}
    failures = []
    for alphabet in ("ascii", "unicode"):
        print(f"new core full membership: {alphabet}", file=sys.stderr, flush=True)
        actual = core_ids(fixture, alphabet)
        if args.save_membership:
            save_membership(result_dir / f"new-{alphabet}.json.gz", actual)
        digests[alphabet] = membership_digests(actual)
        for case in CASES:
            if actual[case] != expected[case]:
                missing = sorted(set(map(tuple, expected[case])) - set(map(tuple, actual[case])))
                extra = sorted(set(map(tuple, actual[case])) - set(map(tuple, expected[case])))
                failures.append({
                    "case": case, "alphabet": alphabet,
                    "expected": len(expected[case]), "actual": len(actual[case]),
                    "missing_first": missing[:10], "extra_first": extra[:10],
                })
            print(
                f"{case} {alphabet}: jq={len(expected[case])} "
                f"new={len(actual[case])} "
                f"{'MISMATCH' if actual[case] != expected[case] else 'ok'}"
            )
    revision = command("git", "rev-parse", "HEAD").stdout.strip()
    jq_version = command("jq", "--version").stdout.strip()
    manifest = {
        "code_revision": revision,
        "jq_version": jq_version,
        "fixture": str(fixture),
        "fixture_metadata_fingerprint": fingerprint,
        "source_files": files_seen,
        "cases": list(CASES),
        "comparisons": len(CASES) * 2,
        "membership_sha256": digests,
        "failures": failures,
        "command": sys.argv,
    }
    (result_dir / "run-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    lines = [
        "# Independent jq membership oracle", "",
        f"New-core code revision: `{revision}`. jq: `{jq_version}`.",
        f"Source files: {files_seen}; fixture metadata fingerprint: `{fingerprint}`.",
        f"Complete ID-set comparisons: {len(CASES) * 2 - len(failures)}/{len(CASES) * 2} passed.",
        "",
        "jq evaluated handwritten predicates directly over each original JSON file.",
        "The new core separately returned complete membership sets under ASCII and Unicode.",
        "This oracle does not use the new core's parser, binder, scanner, index, or cache.",
        "The manifest contains SHA-256 digests of every complete per-case membership set.",
        "Pass `--save-membership` to retain the full expected and actual ID sets as gzip JSON.",
        "",
        "| Case | jq matches | ASCII | Unicode |", "|---|---:|---|---|",
    ]
    for case in CASES:
        bad = {item["alphabet"] for item in failures if item["case"] == case}
        lines.append(
            f"| {case} | {len(expected[case])} | "
            f"{'mismatch' if 'ascii' in bad else 'ok'} | "
            f"{'mismatch' if 'unicode' in bad else 'ok'} |"
        )
    if failures:
        lines.extend(["", "## Mismatches", "", "```json", json.dumps(failures, indent=2), "```"])
    lines.extend(["", "Reproduce: `python3 bench/verify_jq_oracle.py --result-dir /tmp/objekte-jq-oracle`.", ""])
    (result_dir / "REPORT.md").write_text("\n".join(lines))
    return int(bool(failures))


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"oracle failed: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError):
            print(error.stderr, file=sys.stderr)
        sys.exit(2)
