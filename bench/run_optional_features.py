#!/usr/bin/env python3
"""Run the separate schema-free optional-feature workload without compiling.

Build first: cargo build --release -p objekte-search-core --example bench_optional_features
Run: python3 bench/run_optional_features.py --result-dir bench/results/musenalm/optional-features-YYYYMMDD

The primary three runs construct the same unchanged `data/entries` collection
with snapshots disabled. Optional snapshot runs copy the source tree once to an
isolated temporary root, so they cannot change the user's Musenalm cache.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import platform
import shutil
import subprocess
import tempfile
from pathlib import Path


PROJECT = Path(__file__).resolve().parents[1]
MODES = ("disabled", "report", "auto")


def command_output(*command: str) -> str:
    return subprocess.check_output(command, cwd=PROJECT, text=True).strip()


def run_case(binary: Path, root: Path, result_dir: Path, label: str,
             mode: str, snapshot: str, alphabet: str, workers: str,
             query_workers: int | None, cache_bytes: int, skip_queries: bool) -> dict:
    command = [str(binary), "--root", str(root), "--alphabet", alphabet,
               "--inference", mode, "--snapshot-mode", snapshot,
               "--build-workers", workers, "--verified-cache-bytes", str(cache_bytes)]
    if query_workers is not None:
        command.extend(("--query-workers", str(query_workers)))
    if skip_queries:
        command.append("--skip-queries")
    process = subprocess.run(command, cwd=PROJECT, capture_output=True, text=True)
    (result_dir / f"{label}.stdout.json").write_text(process.stdout)
    (result_dir / f"{label}.stderr.log").write_text(process.stderr)
    (result_dir / f"{label}.command.json").write_text(json.dumps(command, indent=2) + "\n")
    if process.returncode:
        raise RuntimeError(f"{label} failed with exit {process.returncode}; see stderr log")
    result = json.loads(process.stdout)
    if result["inference"] != {"disabled": "Disabled", "report": "ReportOnly", "auto": "AutoIndex"}[mode]:
        raise RuntimeError(f"{label} reported the wrong inference mode")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=PROJECT / "examples/musenalm")
    parser.add_argument("--result-dir", type=Path, required=True)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--alphabet", choices=("ascii", "unicode"), default="ascii")
    parser.add_argument("--build-workers", default="auto")
    parser.add_argument("--query-workers", type=int)
    parser.add_argument("--warm-cache", action="store_true", help="also run the 64 MiB warm profile")
    parser.add_argument("--snapshots", action="store_true", help="run isolated rebuild/restore pairs")
    args = parser.parse_args()
    root = args.root.resolve()
    result_dir = args.result_dir.resolve()
    if result_dir.exists() and any(result_dir.iterdir()):
        parser.error(f"result directory is not empty: {result_dir}")
    result_dir.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", PROJECT / "target"))
    binary = (args.binary or target / "release/examples/bench_optional_features").resolve()
    if not binary.is_file():
        parser.error(f"build the release example first: {binary}")
    environment = {
        "git_sha": command_output("git", "rev-parse", "HEAD"),
        "rustc": command_output("rustc", "--version"),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "cpu_count": os.cpu_count(),
        "root": str(root),
        "binary": str(binary),
        "alphabet": args.alphabet,
        "build_workers": args.build_workers,
        "query_workers": args.query_workers,
        "rustflags": os.environ.get("RUSTFLAGS"),
        "cargo_target_dir": os.environ.get("CARGO_TARGET_DIR"),
        "cache_profiles": [0, 64 * 1024 * 1024] if args.warm_cache else [0],
    }
    (result_dir / "environment.json").write_text(json.dumps(environment, indent=2) + "\n")
    results: list[tuple[str, dict]] = []
    for mode in MODES:
        label = f"{mode}-cold"
        result = run_case(binary, root, result_dir, label, mode, "disabled",
                          args.alphabet, args.build_workers, args.query_workers, 0, False)
        results.append((label, result))
        print(f"{label}: ready {result['readiness_ns'] / 1e6:.3f} ms; "
              f"{result['record_count']} records; {len(result['cases'])} cases", flush=True)
        if args.warm_cache:
            label = f"{mode}-warm"
            result = run_case(binary, root, result_dir, label, mode, "disabled",
                              args.alphabet, args.build_workers, args.query_workers, 64 * 1024 * 1024, False)
            results.append((label, result))
            print(f"{label}: ready {result['readiness_ns'] / 1e6:.3f} ms", flush=True)
    if args.snapshots:
        with tempfile.TemporaryDirectory(prefix="objekte-optional-bench-") as temporary:
            isolated = Path(temporary)
            shutil.copytree(root / "data/entries", isolated / "data/entries", copy_function=shutil.copy2)
            for mode in MODES:
                for snapshot in ("rebuild", "read-write"):
                    label = f"{mode}-{snapshot}"
                    result = run_case(binary, isolated, result_dir, label, mode, snapshot,
                                      args.alphabet, args.build_workers, args.query_workers, 0, True)
                    results.append((label, result))
                    print(f"{label}: ready {result['readiness_ns'] / 1e6:.3f} ms; "
                          f"restored={result['snapshot_restored']}", flush=True)
                    if snapshot == "read-write" and not result["snapshot_restored"]:
                        raise RuntimeError(f"{label} unexpectedly rebuilt")
                if mode == "disabled":
                    label = "disabled-to-auto"
                    result = run_case(binary, isolated, result_dir, label, "auto", "read-write",
                                      args.alphabet, args.build_workers, args.query_workers, 0, True)
                    results.append((label, result))
                    print(f"{label}: ready {result['readiness_ns'] / 1e6:.3f} ms; "
                          f"restored={result['snapshot_restored']}", flush=True)
                    if result["snapshot_restored"]:
                        raise RuntimeError("enabling inference restored a Disabled snapshot")
    cold = [value for label, value in results if label.endswith("-cold")]
    fingerprints = {value["corpus_sha256"] for value in cold}
    if len(fingerprints) != 1:
        raise RuntimeError("source metadata changed between inference modes")
    with (result_dir / "summary.csv").open("w", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(("profile", "mode", "readiness_ms", "build_parses", "records",
                         "observed_paths", "admitted_paths", "inferred_index_bytes",
                         "snapshot_bytes", "rss_mib", "peak_rss_mib"))
        for label, value in results:
            report = value["inference_report"] or {}
            writer.writerow((label, value["inference"], value["readiness_ns"] / 1e6,
                             value["build_parses"], value["record_count"],
                             report.get("observed_paths"), report.get("admitted_paths"),
                             report.get("index_bytes_estimate"), value["snapshot_bytes"],
                             (value["rss_after_open_bytes"] or 0) / 1048576,
                             (value["peak_rss_after_open_bytes"] or 0) / 1048576))
    with (result_dir / "queries.csv").open("w", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(("profile", "id", "filter", "fts", "sort_json", "stream_mode",
                         "limit", "offset", "count", "page", "collected_ms",
                         "warm_collected_ms", "stream_ms", "first_data_ms",
                         "first_full_page_ms", "callback_ms", "events", "data_events",
                         "source_opens", "source_parses", "candidates", "scanned_records",
                         "sort_key_evaluations", "sort_target_reads", "sort_key_peak_bytes",
                         "stream_peak_buffered_bytes"))
        for label, value in results:
            for case in value["cases"]:
                counters = case["collected_counters"]
                writer.writerow((
                    label, case["id"], case["filter"], case["fts"],
                    json.dumps(case["sort"], separators=(",", ":")), case["stream_mode"],
                    case["limit"], case["offset"], case["count"], case["page"],
                    case["collected_ns"] / 1e6,
                    None if case["warm_collected_ns"] is None else case["warm_collected_ns"] / 1e6,
                    case["stream_ns"] / 1e6,
                    None if case["first_data_ns"] is None else case["first_data_ns"] / 1e6,
                    None if case["first_full_page_ns"] is None else case["first_full_page_ns"] / 1e6,
                    case["callback_ns"] / 1e6, case["events"], case["data_events"],
                    counters["files_opened"], counters["record_parses"],
                    counters["candidates"], counters["scanned_records"],
                    counters["sort_key_evaluations"], counters["sort_target_reads"],
                    counters["peak_retained_sort_key_bytes"],
                    case["stream_progress"]["peak_buffered_bytes"],
                ))


if __name__ == "__main__":
    main()
