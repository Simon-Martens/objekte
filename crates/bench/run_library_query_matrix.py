#!/usr/bin/env python3
"""Run and validate the fixed library-extension query benchmark matrix."""

import argparse
import json
from pathlib import Path
import subprocess
import time


ROOT = Path(__file__).resolve().parents[2]


def cases(binary: Path, runs: int):
    json_cases = len(json.loads((ROOT / "crates/bench/workloads/musenalm-mix-100.json").read_text()))
    for schema in (True, False):
        for encoding in ("ascii", "utf8_bytes"):
            for budget in (64, 0):
                name = f"json-{'schema' if schema else 'no-schema'}-{encoding}-cache{budget}"
                command = [str(binary / "json-query-mix"), "--repo", "examples/musenalm",
                           "--runs", str(runs), "--bigram-encoding", encoding,
                           "--query-cache-mib", str(budget)]
                if not schema:
                    command.append("--no-schema")
                yield name, command, json_cases
    for budget in (64, 0):
        yield (f"fts-cache{budget}", [str(binary / "fts-query-mix"),
               "examples/musenalm", str(runs), str(budget)], 30)
    for scope in ("original", "relevant"):
        yield (f"sort-{scope}", [str(binary / "json-query-sort-mix"),
               "--repo", "examples/musenalm", "--scope", scope,
               "--runs", str(runs)], 50)
    yield ("json-grep", [str(binary / "json-query-grep-compare"),
           "--repo", "examples/musenalm", "--workload",
           "crates/bench/workloads/musenalm-grep.json", "--runs", str(runs)],
           len(json.loads((ROOT / "crates/bench/workloads/musenalm-grep.json").read_text())))


def validate(name: str, raw: str, expected: int):
    rows = [json.loads(line) for line in raw.splitlines() if line]
    if name == "json-grep":
        if len(rows) != expected or any("matches" not in row for row in rows):
            raise ValueError(f"{name}: expected {expected} complete rows, got {len(rows)}")
        return [{key: row[key] for key in ("collection", "filter", "matches")}
                for row in rows]
    setups = [row for row in rows if row.get("type") == "setup"]
    result = [row for row in rows if row.get("type") == "case"]
    if len(setups) != 1 or setups[0]["cases"] != expected or len(result) != expected:
        raise ValueError(f"{name}: expected one setup and {expected} cases, got {len(rows)} rows")
    if sorted(row["id"] for row in result) != list(range(1, expected + 1)):
        raise ValueError(f"{name}: case IDs are missing or duplicated")
    if any(len(row.get("samples", [])) != setups[0]["runs"] for row in result):
        raise ValueError(f"{name}: incomplete per-case samples")
    if name.startswith("sort-"):
        keys = ("id", "matches", "truncated", "sorted_ids")
    elif name.startswith("fts-"):
        keys = ("id", "matches", "first_id", "fields", "matched_paths")
    else:
        keys = ("id", "matches", "first_id")
    return [{key: row[key] for key in keys if key in row} for row in result]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=7)
    parser.add_argument("--only", help="comma-separated experiment names")
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    wanted = set(args.only.split(",")) if args.only else None
    available = {name for name, _, _ in cases(args.bin_dir, args.runs)}
    if wanted and wanted - available:
        parser.error(f"unknown experiments: {', '.join(sorted(wanted - available))}")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    signatures = {}
    commands = []
    for name, command, expected in cases(args.bin_dir, args.runs):
        if wanted and name not in wanted:
            continue
        started = time.time()
        completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
        (args.output_dir / f"{name}.ndjson").write_text(completed.stdout)
        (args.output_dir / f"{name}.stderr.log").write_text(completed.stderr)
        entry = {"name": name, "command": command, "exit_status": completed.returncode,
                 "started_unix": started, "ended_unix": time.time(),
                 "expected_cases": expected}
        commands.append(entry)
        (args.output_dir / "commands.json").write_text(json.dumps(commands, indent=2) + "\n")
        if completed.returncode:
            raise RuntimeError(f"{name} exited {completed.returncode}; see stderr log")
        signatures[name] = validate(name, completed.stdout, expected)
        (args.output_dir / "signatures.json").write_text(json.dumps(signatures, indent=2) + "\n")
        print(f"validated {name}: {expected} cases", flush=True)


if __name__ == "__main__":
    main()
