#!/usr/bin/env python3
"""Run paired cache-off/on query workloads and retain every measured row."""

import argparse
import json
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[2]
BIN = ROOT / "target/release"


def invoke(command, evidence_dir=None, label=None):
    completed = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
    if evidence_dir is not None:
        (evidence_dir / f"{label}.stderr.log").write_text(completed.stderr)
        (evidence_dir / f"{label}.command.json").write_text(json.dumps({
            "command": command, "exit_status": completed.returncode,
        }, indent=2) + "\n")
    completed.check_returncode()
    return [json.loads(line) for line in completed.stdout.splitlines() if line]


def signature(experiment, rows):
    if experiment.startswith("selective:"):
        return [(row["repetition"], row["ids"], row["total"], row["truncated"])
                for row in rows if row["type"] == "text_step"]
    if experiment in ("sequence", "allocation:sequence"):
        return [
            (row["scenario"], row["label"], row["ids"], row["total"], row["truncated"])
            for row in rows if row["type"] == "step"
        ]
    if experiment == "json100":
        return [(row["id"], row["matches"], row["first_id"]) for row in rows if row["type"] == "case"]
    return [(row["id"], row["matches"]) for row in rows if row["type"] == "case"]


def run_pair(experiment, pair, budget, output, bin_dir, evidence_dir):
    results = {}
    for cache_mib in ([0, budget] if pair % 2 == 0 else [budget, 0]):
        if experiment in ("sequence", "allocation:sequence"):
            command = [str(bin_dir / "query-cache"), "--repo", "examples/musenalm",
                       "--cache-mib", str(cache_mib), "--run", str(pair)]
        elif experiment == "json100":
            command = [str(bin_dir / "json-query-mix"), "--repo", "examples/musenalm",
                       "--runs", "1", "--query-cache-mib", str(cache_mib)]
        elif experiment == "fts30":
            command = [str(bin_dir / "fts-query-mix"), "examples/musenalm", "1", str(cache_mib)]
        else:
            profile = experiment.split(":", 1)[1]
            repo, collection, pattern, encoding = {
                "leipzig": ("leipzig-multilingual", "texts", "ريتشارد", "utf8_bytes"),
                "amazon": ("amazon-reviews-2023", "reviews", "headphones", "ascii"),
                "nyc": ("nyc311", "requests", "Park", "ascii"),
            }[profile]
            command = [str(bin_dir / "query-cache"), "--repo", f"examples/{repo}",
                       "--collection", collection, "--text-pattern", pattern,
                       "--encoding", encoding, "--cache-mib", str(cache_mib), "--run", str(pair)]
        label = f"{experiment.replace(':', '_')}-budget{budget}-pair{pair}-cache{cache_mib}"
        rows = invoke(command, evidence_dir, label)
        results[cache_mib] = rows
        for row in rows:
            output.write(json.dumps({"experiment": experiment, "pair": pair,
                                     "comparison_budget_mib": budget,
                                     "cache_mib": cache_mib, **row}, ensure_ascii=False) + "\n")
        output.flush()
    if signature(experiment, results[0]) != signature(experiment, results[budget]):
        raise RuntimeError(f"{experiment} pair {pair} returned different results")
    print(f"{experiment} budget={budget} MiB pair={pair + 1} passed", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pairs", type=int, default=7)
    parser.add_argument("--output", type=Path,
                        default=ROOT / "crates/bench/results/query-cache-2026-11-03.ndjson")
    parser.add_argument("--skip-suites", action="store_true")
    parser.add_argument("--only-selective", action="store_true")
    parser.add_argument("--only-allocation", action="store_true")
    parser.add_argument("--bin-dir", type=Path, default=BIN)
    parser.add_argument("--evidence-dir", type=Path)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    if args.evidence_dir is not None:
        args.evidence_dir.mkdir(parents=True, exist_ok=True)
    with args.output.open("a" if args.only_selective or args.only_allocation else "w") as output:
        if args.only_allocation:
            for pair in range(args.pairs):
                run_pair("allocation:sequence", pair, 64, output, args.bin_dir, args.evidence_dir)
        elif not args.only_selective:
            for budget in (16, 32, 64):
                for pair in range(args.pairs):
                    run_pair("sequence", pair, budget, output, args.bin_dir, args.evidence_dir)
            if not args.skip_suites:
                for experiment in ("json100", "fts30"):
                    for pair in range(args.pairs):
                        run_pair(experiment, pair, 64, output, args.bin_dir, args.evidence_dir)
        if args.only_selective or (not args.only_allocation and not args.skip_suites):
            for profile in ("leipzig", "amazon", "nyc"):
                for pair in range(args.pairs):
                    run_pair(f"selective:{profile}", pair, 64, output, args.bin_dir,
                             args.evidence_dir)


if __name__ == "__main__":
    main()
