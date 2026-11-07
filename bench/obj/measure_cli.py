#!/usr/bin/env python3
"""Measure release obj startup, queries, and complete JSON output sequentially."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


def fingerprint(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*.json")):
        if any(part in (".git", ".objekte") for part in path.relative_to(root).parts):
            continue
        digest.update(str(path.relative_to(root)).encode())
        digest.update(path.read_bytes())
    return digest.hexdigest()


def measure(binary: Path, output: Path, command: list[str], keep_output: bool) -> dict:
    with tempfile.NamedTemporaryFile(dir=output, delete=False) as sink:
        capture = Path(sink.name)
        start = time.perf_counter_ns()
        result = subprocess.run(
            ["/usr/bin/time", "-f", "PEAK_KB=%M", str(binary), *command],
            stdout=sink,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
        )
        elapsed_ns = time.perf_counter_ns() - start
    events = []
    with capture.open() as source:
        for line in source:
            event = json.loads(line)
            if event["type"] in ("query_complete", "run_complete", "query_error", "run_error"):
                events.append(event)
    if keep_output:
        capture.rename(output / "first-output.ndjson")
    else:
        capture.unlink()
    peak = next((int(line.removeprefix("PEAK_KB=")) for line in result.stderr.splitlines() if line.startswith("PEAK_KB=")), None)
    return {
        "command": [str(binary), *command],
        "exit_code": result.returncode,
        "process_wall_ms": round(elapsed_ns / 1e6, 3),
        "peak_rss_kib": peak,
        "stderr": result.stderr,
        "query": next((event for event in events if event["type"] == "query_complete"), None),
        "run": next((event for event in events if event["type"] == "run_complete"), None),
        "terminal_events": [event["type"] for event in events],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--fixtures", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repetitions", type=int, default=5)
    args = parser.parse_args()
    if args.repetitions < 1:
        parser.error("repetitions must be positive")
    binary = args.binary.resolve(strict=True)
    fixtures = args.fixtures.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    results = []
    for alphabet in ("ascii", "unicode"):
        for dataset in ("general", "large"):
            root = fixtures / dataset
            snapshot = output / f"snapshot-{alphabet}-{dataset}"
            common = ["--path", str(root), "--index-alphabet", alphabet]
            scenarios = [
                ("fresh-write", [*common, f"--write-index={snapshot}"]),
                ("restore", [*common, f"--load-index={snapshot}"]),
            ]
            if dataset == "general":
                scenarios += [
                    ("selective-fts", [*common, f"--load-index={snapshot}", "--fts", "Goethe"]),
                    ("dynamic-filter", [*common, f"--load-index={snapshot}", 'title ~ "Goethe"']),
                    ("numeric-scan", [*common, f"--load-index={snapshot}", "year >= 1800"]),
                    ("finite-sort", [*common, f"--load-index={snapshot}", "--limit", "5", "--count", "year >= 1800 SORT BY year DESC"]),
                ]
            else:
                scenarios += [
                    ("all-results", [*common, f"--load-index={snapshot}", "--fts", "export-match"]),
                    ("computed-sort-all", [*common, f"--load-index={snapshot}", 'kind = "export-match" SORT BY ordinal DESC']),
                    ("computed-sort-page", [*common, f"--load-index={snapshot}", "--limit", "50", "--count", 'kind = "export-match" SORT BY ordinal DESC']),
                ]
            for name, command in scenarios:
                samples = []
                for index in range(args.repetitions):
                    sample = measure(binary, output, command, keep_output=False)
                    if sample["exit_code"] != 0 or sample["run"] is None or any(t.endswith("error") for t in sample["terminal_events"]):
                        raise RuntimeError(f"{alphabet}/{dataset}/{name}/{index}: {sample['stderr']} {sample['terminal_events']}")
                    samples.append(sample)
                results.append({"alphabet": alphabet, "dataset": dataset, "scenario": name, "samples": samples})
                wall = statistics.median(sample["process_wall_ms"] for sample in samples)
                query = statistics.median(sample["query"]["query_wall_ms"] for sample in samples if sample["query"]) if samples[0]["query"] else None
                query_label = f"{query:.3f} ms" if query is not None else "—"
                print(f"{alphabet:7} {dataset:7} {name:19} process={wall:.3f} ms query={query_label}", flush=True)
    manifest = {
        "code_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "binary": str(binary),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "machine": platform.uname()._asdict(),
        "cpu_affinity": sorted(os.sched_getaffinity(0)),
        "fixture_sha256": {name: fingerprint(fixtures / name) for name in ("general", "large")},
        "filesystem_cache": "warm/unspecified; repeated sequential process invocations without cache dropping",
        "repetitions": args.repetitions,
        "results": results,
    }
    (output / "measurements.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
