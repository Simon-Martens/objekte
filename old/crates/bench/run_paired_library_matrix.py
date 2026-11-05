#!/usr/bin/env python3
"""Pair the fixed query matrix against pinned executables on one source tree."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess

from run_library_query_matrix import ROOT, cases, validate


def digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def run(name: str, command: list[str], expected: int, rounds: int, path: Path) -> list[dict]:
    command = ["taskset", "-c", "0-15", *command]
    completed = subprocess.run(command, cwd=ROOT,
                               env=dict(os.environ, RAYON_NUM_THREADS="16"),
                               capture_output=True, text=True)
    path.with_suffix(".stdout.ndjson").write_text(completed.stdout)
    path.with_suffix(".stderr.log").write_text(completed.stderr)
    path.with_suffix(".command.json").write_text(json.dumps({
        "command": command, "rayon_threads": 16,
        "exit_status": completed.returncode,
    }, indent=2) + "\n")
    if completed.returncode:
        raise RuntimeError(f"{name}: {path.name} exited {completed.returncode}")
    rows = [json.loads(line) for line in completed.stdout.splitlines() if line]
    validate(name, completed.stdout, expected)
    if name == "json-grep":
        if any(row["runs"] != rounds for row in rows):
            raise RuntimeError(f"{name}: incomplete runs")
    return rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", type=Path, required=True, help="directory of pinned release binaries")
    parser.add_argument("--candidate", type=Path, required=True, help="directory of candidate release binaries")
    parser.add_argument("--expected-input", type=Path, required=True, help="pinned corpora.json")
    parser.add_argument("--actual-input", type=Path, required=True, help="fresh corpus fingerprint file")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pairs", type=int, default=1)
    parser.add_argument("--rounds", type=int, default=7)
    parser.add_argument("--only", help="comma-separated experiment names")
    args = parser.parse_args()
    if args.pairs < 1 or args.rounds < 1:
        parser.error("pairs and rounds must be positive")
    expected_input = json.loads(args.expected_input.read_text())
    actual_input = json.loads(args.actual_input.read_text())
    if expected_input != actual_input:
        raise RuntimeError("corpus source, metadata or configuration changed from baseline")
    reference = args.reference.resolve(strict=True)
    candidate = args.candidate.resolve(strict=True)
    wanted = set(args.only.split(",")) if args.only else None
    reference_cases = list(cases(reference, args.rounds))
    candidate_cases = list(cases(candidate, args.rounds))
    available = {name for name, _, _ in reference_cases}
    if wanted and wanted - available:
        parser.error(f"unknown experiments: {', '.join(sorted(wanted - available))}")
    args.output.mkdir(parents=True, exist_ok=False)
    manifest = {
        "reference": str(reference), "candidate": str(candidate),
        "reference_binaries": {p.name: digest(p) for p in reference.iterdir() if p.is_file()},
        "candidate_binaries": {p.name: digest(p) for p in candidate.iterdir() if p.is_file()},
        "actual_input_sha256": digest(args.actual_input),
        "expected_input_sha256": digest(args.expected_input),
        "workloads": {p.name: digest(p) for p in (ROOT / "crates/bench/workloads").glob("musenalm*.json")},
        "pairs": args.pairs, "rounds": args.rounds,
        "affinity": "0-15", "rayon_threads": 16,
        "experiments": [name for name, _, _ in reference_cases if not wanted or name in wanted],
    }
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    summaries = {}
    for position, ((name, ref_command, expected), (_, cand_command, _)) in enumerate(zip(reference_cases, candidate_cases)):
        if wanted and name not in wanted:
            continue
        folder = args.output / name
        folder.mkdir()
        pairs = []
        for number in range(1, args.pairs + 1):
            order = ("reference", "candidate") if (number + position) % 2 else ("candidate", "reference")
            rows = {}
            for label in order:
                command = ref_command if label == "reference" else cand_command
                print(f"{name} pair {number}/{args.pairs} {label}", flush=True)
                rows[label] = run(name, command, expected, args.rounds, folder / f"{number:02d}-{label}")
            ref_signature = validate(name, "\n".join(json.dumps(row) for row in rows["reference"]), expected)
            cand_signature = validate(name, "\n".join(json.dumps(row) for row in rows["candidate"]), expected)
            if ref_signature != cand_signature:
                raise RuntimeError(f"{name}: semantic signature mismatch in pair {number}")
            pairs.append(rows)
        first = pairs[0]["reference"]
        offset = 0 if name == "json-grep" else 1
        if name == "json-grep":
            metrics = ("warm_bigram_median_ms", "grep_median_ms")
        elif name.startswith("sort-"):
            metrics = ("baseline_ms", "sorted_ms")
        else:
            metrics = ("median_ms",)
        cases_summary = []
        for index in range(expected):
            current = {}
            for metric in metrics:
                ref_values = [pair["reference"][index + offset][metric] for pair in pairs]
                cand_values = [pair["candidate"][index + offset][metric] for pair in pairs]
                current[metric] = {
                    "reference_median": statistics.median(ref_values),
                    "candidate_median": statistics.median(cand_values),
                    "paired_deltas": [c - r for r, c in zip(ref_values, cand_values)],
                }
            ref_row = first[index + offset]
            current["id"] = ref_row.get("id", index + 1)
            current["query"] = ref_row.get("query", ref_row.get("filter"))
            for work in (
                "scans", "source_reads", "parses", "fts_candidates",
                "cache_hits", "ordered_hits", "baseline_reads", "baseline_parses",
                "sorted_reads", "sorted_parses", "grep_candidates",
            ):
                if work in ref_row:
                    current[work + "_equal"] = all(pair["reference"][index + offset][work]
                                                        == pair["candidate"][index + offset][work]
                                                        for pair in pairs)
            cases_summary.append(current)
        summaries[name] = {"cases": cases_summary, "expected_cases": expected,
                           "reference_setup": first[0] if offset else None,
                           "candidate_setup": pairs[0]["candidate"][0] if offset else None}
        (args.output / "summary.json").write_text(json.dumps(summaries, indent=2, sort_keys=True) + "\n")
        print(f"validated {name}: {expected} signatures and {args.pairs} pair(s)", flush=True)


if __name__ == "__main__":
    main()
