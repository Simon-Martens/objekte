#!/usr/bin/env python3
"""Pair the current JSON query mix against the preserved production baseline."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess

from fingerprint_corpora import fingerprint


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run(binary: Path, repo: Path, workload: Path, cases: int, rounds: int, budget: int, destination: Path) -> list[dict]:
    command = [
        "taskset", "-c", "0-15", str(binary), "--repo", str(repo),
        "--runs", str(rounds), "--bigram-encoding", "ascii",
        "--query-cache-mib", str(budget),
        "--workload", str(workload),
    ]
    completed = subprocess.run(
        command, env=dict(os.environ, RAYON_NUM_THREADS="16"),
        capture_output=True, check=False,
    )
    destination.with_suffix(".stdout.ndjson").write_bytes(completed.stdout)
    destination.with_suffix(".stderr.log").write_bytes(completed.stderr)
    destination.with_suffix(".command.json").write_text(
        json.dumps({"command": command, "exit_status": completed.returncode}, indent=2) + "\n"
    )
    if completed.returncode:
        raise RuntimeError(f"{command} failed with status {completed.returncode}")
    rows = [json.loads(line) for line in completed.stdout.splitlines()]
    if len(rows) != cases + 1 or rows[0].get("cases") != cases:
        raise RuntimeError(f"{command} returned {len(rows)} rows, expected {cases + 1}")
    if any(len(row.get("samples", [])) != rounds for row in rows[1:]):
        raise RuntimeError(f"{command} returned incomplete samples")
    return rows


def signature(row: dict) -> tuple:
    return tuple(row.get(key) for key in ("id", "kind", "filter", "matches", "first_id"))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--repo", type=Path, default=Path("examples/musenalm"))
    parser.add_argument("--workload", type=Path, default=Path("crates/bench/workloads/musenalm-mix-100.json"))
    parser.add_argument("--expected-input", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--pairs", type=int, default=3)
    parser.add_argument("--rounds", type=int, default=7)
    args = parser.parse_args()
    if args.pairs < 1 or args.rounds < 1:
        parser.error("pairs and rounds must be positive")
    args.output.mkdir(parents=True, exist_ok=False)
    reference = args.reference.resolve(strict=True)
    candidate = args.candidate.resolve(strict=True)
    repo = args.repo.resolve(strict=True)
    workload = args.workload.resolve(strict=True)
    cases = len(json.loads(workload.read_text()))
    if cases < 1:
        raise RuntimeError("empty query workload")
    actual_input = fingerprint(repo)
    expected_input = json.loads(args.expected_input.read_text())["musenalm"]
    if actual_input != expected_input:
        raise RuntimeError("Musenalm source or metadata differs from the pinned baseline")
    (args.output / "manifest.json").write_text(json.dumps({
        "reference": {"path": str(reference), "sha256": sha256(reference)},
        "candidate": {"path": str(candidate), "sha256": sha256(candidate)},
        "input": actual_input, "pairs": args.pairs, "rounds": args.rounds,
        "workload": {"path": str(workload), "sha256": sha256(workload), "cases": cases},
        "rayon_threads": 16, "cpu_affinity": "0-15", "encoding": "ascii",
    }, indent=2, sort_keys=True) + "\n")
    summary = {}
    for budget in (0, 64):
        directory = args.output / f"cache{budget}"
        directory.mkdir()
        for label, binary in (("reference", reference), ("candidate", candidate)):
            run(binary, repo, workload, cases, args.rounds, budget, directory / f"warmup-{label}")
        pairs = []
        for number in range(args.pairs):
            order = ("reference", "candidate") if number % 2 == 0 else ("candidate", "reference")
            rows = {}
            for label in order:
                print(f"cache{budget} pair {number + 1}/{args.pairs} {label}", flush=True)
                rows[label] = run(
                    reference if label == "reference" else candidate,
                    repo, workload, cases, args.rounds, budget, directory / f"{number + 1:02d}-{label}",
                )
            for baseline, current in zip(rows["reference"][1:], rows["candidate"][1:]):
                if signature(baseline) != signature(current):
                    raise RuntimeError(f"semantic mismatch in cache{budget} case {baseline['id']}")
            pairs.append(rows)
        summary[f"cache{budget}"] = []
        for index in range(1, cases + 1):
            baseline = [pair["reference"][index] for pair in pairs]
            current = [pair["candidate"][index] for pair in pairs]
            summary[f"cache{budget}"].append({
                "id": index,
                "filter": baseline[0]["filter"],
                "reference_median_ms": statistics.median(row["median_ms"] for row in baseline),
                "candidate_median_ms": statistics.median(row["median_ms"] for row in current),
                "paired_deltas_ms": [c["median_ms"] - b["median_ms"] for b, c in zip(baseline, current)],
                "reference_work": [
                    {key: row[key] for key in ("scans", "source_reads", "parses")}
                    for row in baseline
                ],
                "candidate_work": [
                    {key: row[key] for key in ("scans", "source_reads", "parses")}
                    for row in current
                ],
            })
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    print(f"validated {args.pairs * 4} processes and {cases} case signatures per pair")


if __name__ == "__main__":
    main()
