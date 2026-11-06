#!/usr/bin/env python3
"""Benchmark fresh Musenalm index builds in four configurations."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import resource
import statistics
import subprocess
import sys
from decimal import Decimal
from pathlib import Path

from schema_evidence import canonical_registrations, compare_schema_identity, schema_identity, validate_schema_artifact


ROOT = Path(__file__).resolve().parent.parent


def fixture_fingerprints() -> dict[str, str]:
    fixture = ROOT / "examples/musenalm"
    schema_hash = hashlib.sha256()
    source_hash = hashlib.sha256()
    schema_paths = sorted((fixture / ".objekte/collections").glob("*.schema.json"))
    if not schema_paths:
        raise ValueError("Musenalm schema inventory is missing")
    for schema in schema_paths:
        schema_hash.update(str(schema.relative_to(fixture)).encode())
        schema_hash.update(schema.read_bytes())
        declaration = json.loads(schema.read_text())["x-objekte"]
        for path in sorted((fixture / declaration["path"]).glob("*.json")):
            info = path.stat()
            source_hash.update(str(path.relative_to(fixture)).encode())
            source_hash.update(info.st_size.to_bytes(8, "little"))
            source_hash.update(info.st_mtime_ns.to_bytes(16, "little", signed=True))
    return {"schema": schema_hash.hexdigest(), "source_metadata": source_hash.hexdigest()}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--result-dir", type=Path)
    parser.add_argument("--baseline", type=Path, required=True, help="explicit gate.json or build baseline JSON")
    parser.add_argument("--comparison", choices=("repair", "historical", "casefold", "parallel"), required=True)
    parser.add_argument("--build-workers", default="auto", help="auto or a positive count within available CPUs")
    parser.add_argument("--snapshot-modes", default="disabled,rebuild", help="comma-separated disabled,rebuild subset")
    parser.add_argument("--establish-baseline", action="store_true")
    args = parser.parse_args()
    if args.repetitions < 3:
        parser.error("at least three fresh-process repetitions are required")
    if args.build_workers != "auto" and (not args.build_workers.isdecimal() or int(args.build_workers) < 1):
        parser.error("build workers must be auto or a positive integer")
    snapshot_modes = tuple(args.snapshot_modes.split(","))
    if not snapshot_modes or len(set(snapshot_modes)) != len(snapshot_modes) or any(
        mode not in ("disabled", "rebuild") for mode in snapshot_modes
    ):
        parser.error("snapshot modes must be a unique comma-separated disabled,rebuild subset")
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    fingerprints = fixture_fingerprints()
    jq_fixture = json.loads((ROOT / "bench/baselines/musenalm-jq-counts.json").read_text())
    if fingerprints["source_metadata"] != jq_fixture["fixture_metadata_fingerprint"]:
        raise ValueError("Musenalm source metadata differs from the independent jq fixture")
    result_dir = (args.result_dir or ROOT / "bench/results/musenalm" / f"build-{revision}").resolve()
    result_dir.mkdir(parents=True, exist_ok=True)
    binary = ROOT / "target/release/examples/bench_build_musenalm"
    with (result_dir / "build.log").open("w") as log:
        completed = subprocess.run(["cargo", "build", "--release", "--example", "bench_build_musenalm"], cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
    if completed.returncode:
        print(f"build benchmark compilation failed: {result_dir / 'build.log'}", file=sys.stderr)
        return completed.returncode
    configurations = tuple((alphabet, snapshot) for alphabet in ("ascii", "unicode") for snapshot in snapshot_modes)
    samples: dict[str, list[dict]] = {f"{alphabet}-{snapshot}": [] for alphabet, snapshot in configurations}
    with (result_dir / "measurements.jsonl").open("w") as output:
        for repetition in range(args.repetitions):
            for alphabet, snapshot in configurations:
                command = [str(binary), "--alphabet", alphabet, "--snapshot-mode", snapshot,
                           "--build-workers", args.build_workers,
                           "--schema-manifest-path", str(result_dir / "schema-manifest.json")]
                before_usage = resource.getrusage(resource.RUSAGE_CHILDREN)
                completed = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
                after_usage = resource.getrusage(resource.RUSAGE_CHILDREN)
                if completed.returncode:
                    raise RuntimeError(f"{command}: {completed.stderr}")
                sample = json.loads(completed.stdout)
                names = set(sample["schema_collections"])
                if sample["schema_collections"] != sorted(names):
                    raise ValueError("build sample has duplicate or unsorted schema collections")
                digest, schema_error = schema_identity(result_dir, names, sample["schema_manifest_digest"])
                if schema_error:
                    raise ValueError(schema_error)
                if any(previous["schema_manifest_digest"] != digest
                       for group in samples.values() for previous in group):
                    raise ValueError("schema set changed between build samples")
                if any(canonical_registrations(previous) != canonical_registrations(sample)
                       for group in samples.values() for previous in group):
                    raise ValueError("registrations changed between build samples")
                key = f"{alphabet}-{snapshot}"
                if any(previous.get("index_signature_sha256") != sample.get("index_signature_sha256")
                       for previous in samples[key]):
                    raise ValueError(f"semantic index signature changed between {key} samples")
                sample["repetition"] = repetition
                sample["command"] = command
                sample["process_user_seconds"] = after_usage.ru_utime - before_usage.ru_utime
                sample["process_system_seconds"] = after_usage.ru_stime - before_usage.ru_stime
                sample["average_busy_cpus"] = (
                    (sample["process_user_seconds"] + sample["process_system_seconds"])
                    / (sample["wall_ns"] / 1e9)
                )
                if sample["records"] != 148486 or sample["build_parses"] != 148486 or sample.get("failed_parses", 0):
                    raise ValueError(f"incomplete build: {sample}")
                samples[key].append(sample)
                output.write(json.dumps(sample) + "\n")
                output.flush()
                print(f"{key} #{repetition + 1}: {sample['wall_ns'] / 1e9:.3f}s", flush=True)
    summary = {}
    for name, group in samples.items():
        summary[name] = {
            "median_wall_ns": int(statistics.median(sample["wall_ns"] for sample in group)),
            "median_build_ns": int(statistics.median(sample["build_ns"] for sample in group)),
            "median_snapshot_write_ns": int(statistics.median(sample["snapshot_write_ns"] for sample in group)),
            "median_peak_resident_bytes": int(statistics.median(sample["peak_resident_bytes"] for sample in group)),
            "median_resident_bytes": int(statistics.median(sample["resident_bytes"] for sample in group)),
            "median_discovery_ns": int(statistics.median(sample.get("discovery_ns", 0) for sample in group)),
            "median_extraction_index_ns": int(statistics.median(sample.get("extraction_index_ns", 0) for sample in group)),
            "median_finalization_ns": int(statistics.median(sample.get("finalization_ns", 0) for sample in group)),
            "median_process_user_seconds": statistics.median(sample["process_user_seconds"] for sample in group),
            "median_process_system_seconds": statistics.median(sample["process_system_seconds"] for sample in group),
            "median_average_busy_cpus": statistics.median(sample["average_busy_cpus"] for sample in group),
            "min_wall_ns": min(sample["wall_ns"] for sample in group),
            "max_wall_ns": max(sample["wall_ns"] for sample in group),
            "resolved_build_workers": group[0]["resolved_build_workers"],
            "median_peak_build_admitted_bytes": int(statistics.median(sample["peak_build_admitted_bytes"] for sample in group)),
            "median_inventory_heap_bytes": int(statistics.median(sample.get("inventory_heap_bytes", 0) for sample in group)),
            "median_diagnostic_heap_bytes": int(statistics.median(sample.get("diagnostic_heap_bytes", 0) for sample in group)),
            "index_stores": group[0].get("index_stores"),
            "distinct_gram_keys": group[0].get("distinct_gram_keys"),
            "bigram_heap_bytes": group[0].get("bigram_heap_bytes"),
            "snapshot_bytes": group[0]["snapshot_bytes"],
            "index_signature_sha256": group[0].get("index_signature_sha256"),
            "samples": len(group),
        }
    for alphabet in ("ascii", "unicode"):
        signatures = {row["index_signature_sha256"] for name, row in summary.items() if name.startswith(alphabet)}
        if len(signatures) > 1:
            raise ValueError(f"semantic index signature differs by snapshot mode: {alphabet}")
    environment = {
        "cpu_model": next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), "unknown"),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "revision": revision,
        "requested_build_workers": args.build_workers,
        "resolved_build_workers": sorted({sample["resolved_build_workers"] for group in samples.values() for sample in group}),
        "snapshot_modes": snapshot_modes,
        "build_flags": {key: value for key, value in os.environ.items()
                        if key in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET")
                        or key.startswith("CARGO_PROFILE_RELEASE_")},
        "affinity": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
        "governor": Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").read_text().strip()
        if Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").exists() else None,
    }
    failures = []
    baseline_path = args.baseline.resolve()
    if args.establish_baseline:
        baseline_path.parent.mkdir(parents=True, exist_ok=True)
        if baseline_path.exists():
            raise FileExistsError(f"baseline exists: {baseline_path}")
        baseline_path.write_text(json.dumps({"environment": environment, "fingerprints": fingerprints,
                                             "schema_manifest_json": (result_dir / "schema-manifest.json").read_text(),
                                             "schema_collections": sorted(names),
                                             "registrations": sample["registrations"], "summary": summary}, indent=2) + "\n")
    else:
        baseline = json.loads((baseline_path / "gate.json" if baseline_path.is_dir() else baseline_path).read_text())
        if baseline_path.is_dir():
            failures.extend(compare_schema_identity(
                baseline_path, result_dir,
                set(baseline["schema_collections"]) if "schema_collections" in baseline else None, names,
                baseline.get("schema_manifest_digest"), digest,
            ))
        else:
            old_artifact_json = baseline.get("schema_manifest_json")
            if old_artifact_json is None:
                failures.append("baseline unverifiable schema baseline: missing schema manifest")
            else:
                try:
                    old_artifact = json.loads(old_artifact_json, parse_float=Decimal)
                except (TypeError, json.JSONDecodeError):
                    old_artifact = None
                _, old_error = validate_schema_artifact(old_artifact, set(baseline.get("schema_collections", [])))
                if old_error:
                    failures.append(f"baseline {old_error}")
                elif old_artifact["sha256"] != digest:
                    failures.append("semantic schema differs from baseline")
        old_registrations = baseline.get("registrations")
        if old_registrations is None and baseline_path.is_dir():
            registration_path = baseline_path / "registration-manifest.json"
            if registration_path.is_file():
                try:
                    old_registrations = json.loads(registration_path.read_text())["registrations"]
                except (OSError, json.JSONDecodeError, KeyError):
                    failures.append("unverifiable registration baseline")
        if old_registrations is None:
            failures.append("unverifiable registration baseline")
        elif canonical_registrations({"registrations": old_registrations}) != canonical_registrations(sample):
            failures.append("semantic registrations differ from build baseline")
        expected_revision = {"repair": "92cdb92b19", "historical": "401b16eea8", "casefold": "20261604be"}.get(args.comparison)
        if expected_revision and not baseline["environment"]["revision"].startswith(expected_revision):
            failures.append(f"{args.comparison} comparison uses wrong baseline revision")
        if baseline["environment"]["revision"] == revision:
            failures.append("baseline and current code revisions are identical")
        if baseline["fingerprints"]["source_metadata"] != fingerprints["source_metadata"]:
            failures.append("source metadata fingerprint differs from build baseline")
        if baseline["environment"]["rustc"] != environment["rustc"]:
            failures.append("compiler differs from build baseline")
        if baseline["environment"].get("governor", "powersave") != environment["governor"]:
            failures.append("CPU governor differs from declared build baseline")
        if baseline["environment"].get("affinity", list(range(32))) != environment["affinity"]:
            failures.append("CPU affinity differs from declared build baseline")
        if environment["build_flags"]:
            failures.append("nondefault build flags are not recorded in the baseline")
        if baseline["environment"]["cpu_model"] != environment["cpu_model"]:
            failures.append("CPU model differs from build baseline")
        if baseline["environment"]["cpu_model"] == environment["cpu_model"]:
            for name, current in summary.items():
                previous = baseline["summary"][name]
                if current["median_wall_ns"] > max(previous["median_wall_ns"] * 1.5, previous["median_wall_ns"] + 2_000_000_000):
                    failures.append(f"{name}: build {current['median_wall_ns'] / 1e9:.3f}s exceeds baseline {previous['median_wall_ns'] / 1e9:.3f}s")
                if current["median_peak_resident_bytes"] > max(previous["median_peak_resident_bytes"] * 1.3, previous["median_peak_resident_bytes"] + 64 * 1024 * 1024):
                    failures.append(f"{name}: peak RSS exceeds baseline")
                if current["snapshot_bytes"] and current["snapshot_bytes"] > previous["snapshot_bytes"] * 1.1:
                    failures.append(f"{name}: snapshot size exceeds baseline")
    report = {"comparison": args.comparison, "environment": environment, "fingerprints": fingerprints,
              "schema_manifest_digest": digest, "schema_collections": sorted(names),
              "registrations": sample["registrations"], "summary": summary,
              "baseline": str(baseline_path), "baseline_code_revision": None if args.establish_baseline else baseline["environment"]["revision"],
              "failures": failures}
    (result_dir / "gate.json").write_text(json.dumps(report, indent=2) + "\n")
    lines = [
        "# Musenalm fresh index builds", "",
        f"Code revision: `{revision}`. Each configuration ran in {args.repetitions} separate processes.",
        f"Both alphabets were measured with snapshot modes: {', '.join(snapshot_modes)}.",
        "All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.",
        "", "| Configuration | Workers | Wall s | Range s | CPU s | Busy CPUs | Discovery s | Extraction/index s | Finalization s | Snapshot write s | Resident MiB | Peak RSS MiB | Peak admitted MiB | Snapshot MiB |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for name, row in summary.items():
        snapshot_mib = "—" if row["snapshot_bytes"] is None else f"{row['snapshot_bytes'] / 1048576:.1f}"
        lines.append(
            f"| {name} | {row['resolved_build_workers']} | {row['median_wall_ns'] / 1e9:.3f} | "
            f"{row['min_wall_ns'] / 1e9:.3f}–{row['max_wall_ns'] / 1e9:.3f} | "
            f"{row['median_process_user_seconds'] + row['median_process_system_seconds']:.3f} | "
            f"{row['median_average_busy_cpus']:.2f} | {row['median_discovery_ns'] / 1e9:.3f} | "
            f"{row['median_extraction_index_ns'] / 1e9:.3f} | {row['median_finalization_ns'] / 1e9:.3f} | "
            f"{row['median_snapshot_write_ns'] / 1e9:.3f} | {row['median_resident_bytes'] / 1048576:.1f} | "
            f"{row['median_peak_resident_bytes'] / 1048576:.1f} | "
            f"{row['median_peak_build_admitted_bytes'] / 1048576:.1f} | {snapshot_mib} |"
        )
    with (result_dir / "build-results.csv").open("w", newline="") as output:
        writer = csv.writer(output)
        writer.writerow(("configuration", "workers", "median_wall_ms", "min_wall_ms", "max_wall_ms",
                         "median_cpu_seconds", "median_busy_cpus", "median_peak_rss_bytes",
                         "median_peak_admitted_bytes", "snapshot_bytes"))
        for name, row in summary.items():
            writer.writerow((name, row["resolved_build_workers"], row["median_wall_ns"] / 1e6,
                             row["min_wall_ns"] / 1e6, row["max_wall_ns"] / 1e6,
                             row["median_process_user_seconds"] + row["median_process_system_seconds"],
                             row["median_average_busy_cpus"], row["median_peak_resident_bytes"],
                             row["median_peak_build_admitted_bytes"], row["snapshot_bytes"]))
    lines += ["", f"Baseline: `{baseline_path}`. Failures: {len(failures)}.", "", *[f"- {failure}" for failure in failures]]
    report_text = "\n".join(lines).rstrip() + "\n"
    (result_dir / "REPORT.md").write_text(report_text)
    print(report_text)
    for failure in failures:
        print(failure, file=sys.stderr)
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
