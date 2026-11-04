#!/usr/bin/env python3
"""Run the existing FTS mix in alternating reference/candidate processes."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys

from fingerprint_corpora import fingerprint


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run(binary: Path, repo: Path, rounds: int, budget: int, destination: Path) -> list[dict]:
    command = ["taskset", "-c", "0-15", str(binary), str(repo), str(rounds), str(budget)]
    environment = dict(os.environ, RAYON_NUM_THREADS="16")
    completed = subprocess.run(command, env=environment, capture_output=True, check=False)
    destination.with_suffix(".stdout.ndjson").write_bytes(completed.stdout)
    destination.with_suffix(".stderr.log").write_bytes(completed.stderr)
    destination.with_suffix(".command.json").write_text(
        json.dumps({"command": command, "exit_status": completed.returncode}, indent=2) + "\n"
    )
    if completed.returncode:
        raise RuntimeError(f"{command} failed with status {completed.returncode}")
    rows = [json.loads(line) for line in completed.stdout.splitlines()]
    if len(rows) != 31 or rows[0].get("cases") != 30:
        raise RuntimeError(f"{command} returned {len(rows)} rows, expected a header and 30 cases")
    if any(len(row.get("samples", [])) != rounds for row in rows[1:]):
        raise RuntimeError(f"{command} returned incomplete per-case samples")
    return rows


def signature(row: dict) -> tuple:
    return tuple(json.dumps(row.get(key), sort_keys=True) for key in (
        "id", "kind", "query", "field", "type", "matches", "first_id", "fields", "matched_paths"
    ))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--repo", type=Path, default=Path("examples/musenalm"))
    parser.add_argument("--expected-input", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pairs", type=int, default=5)
    parser.add_argument("--rounds", type=int, default=7)
    args = parser.parse_args()
    if args.pairs < 1 or args.rounds < 1:
        parser.error("pairs and rounds must be positive")
    args.output.mkdir(parents=True, exist_ok=False)
    reference = args.reference.resolve(strict=True)
    candidate = args.candidate.resolve(strict=True)
    repo = args.repo.resolve(strict=True)
    actual_input = fingerprint(repo)
    expected_input = json.loads(args.expected_input.read_text())["musenalm"]
    if actual_input != expected_input:
        raise RuntimeError("Musenalm content, source metadata, or configuration changed")
    manifest = {
        "reference": {"path": str(reference), "sha256": sha256(reference)},
        "candidate": {"path": str(candidate), "sha256": sha256(candidate)},
        "repo": str(repo), "input": actual_input,
        "rounds": args.rounds, "pairs": args.pairs,
        "rayon_threads": 16, "cpu_affinity": "0-15",
        "warmup": "one full process per binary and cache budget",
    }
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    summary = {}
    for budget in (0, 64):
        directory = args.output / f"cache{budget}"
        directory.mkdir()
        for label, binary in (("reference", reference), ("candidate", candidate)):
            run(binary, repo, args.rounds, budget, directory / f"warmup-{label}")
        pairs = []
        for number in range(args.pairs):
            order = ("reference", "candidate") if number % 2 == 0 else ("candidate", "reference")
            rows = {}
            for label in order:
                print(f"cache{budget} pair {number + 1}/{args.pairs} {label}", flush=True)
                rows[label] = run(
                    reference if label == "reference" else candidate,
                    repo, args.rounds, budget, directory / f"{number + 1:02d}-{label}",
                )
            for first, second in zip(rows["reference"][1:], rows["candidate"][1:]):
                if signature(first) != signature(second):
                    raise RuntimeError(f"semantic mismatch in cache{budget} pair {number + 1} case {first['id']}")
            pairs.append(rows)
        cases = []
        for index in range(1, 31):
            reference_ms = [pair["reference"][index]["median_ms"] for pair in pairs]
            candidate_ms = [pair["candidate"][index]["median_ms"] for pair in pairs]
            deltas = [candidate - reference for candidate, reference in zip(candidate_ms, reference_ms)]
            reference_work = [pair["reference"][index]["fts_candidates"] for pair in pairs]
            candidate_work = [pair["candidate"][index]["fts_candidates"] for pair in pairs]
            cases.append({
                "id": index,
                "query": pairs[0]["reference"][index]["query"],
                "reference_median_ms": statistics.median(reference_ms),
                "candidate_median_ms": statistics.median(candidate_ms),
                "paired_deltas_ms": deltas,
                "candidate_count_reference": reference_work,
                "candidate_count_candidate": candidate_work,
            })
        summary[f"cache{budget}"] = cases
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    print(f"validated {args.pairs * 2 * 2} measured processes and all 30 case signatures per budget")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(error, file=sys.stderr)
        raise
