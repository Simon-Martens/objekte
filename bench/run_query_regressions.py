#!/usr/bin/env python3
"""Run the 150-sample iteration gate or the full 3,000-sample query gate."""

from __future__ import annotations

import argparse
import csv
import json
import os
import platform
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

from schema_evidence import canonical_registrations, compare_schema_identity, schema_identity


ROOT = Path(__file__).resolve().parent.parent
JQ_COUNTS = ROOT / "bench/baselines/musenalm-jq-counts.json"
PROFILES = ("ascii-off", "ascii-warm", "unicode-off", "unicode-warm")


def rows(path: Path) -> dict[tuple[str, str], dict[str, str]]:
    with path.open(newline="") as file:
        values = list(csv.DictReader(file))
    result = {(row["case_id"], row["profile"]): row for row in values}
    if len(result) != len(values):
        raise ValueError(f"duplicate query rows in {path}")
    return result


def compatibility_failures(baseline: Path, current: Path, old: dict, new: dict) -> list[str]:
    failures = []
    for key in ("corpus_fingerprint", "compiler",
                "source_cache_bytes", "decoded_cache_bytes", "query_workers"):
        if old[key] != new[key]:
            failures.append(f"{key} differs from baseline")
    failures.extend(compare_schema_identity(
        baseline, current,
        {registration["name"] for registration in old["registrations"]},
        {registration["name"] for registration in new["registrations"]},
        old.get("schema_manifest_digest"), new.get("schema_manifest_digest"),
    ))
    old_profiles = dict(old["profiles"])
    if any(old_profiles.get(name) != profile for name, profile in new["profiles"]):
        failures.append("selected cache/alphabet profiles differ from baseline")
    if canonical_registrations(old) != canonical_registrations(new):
        failures.append("semantic registrations differ from baseline")
    if json.loads((baseline / "queries.json").read_text()) != json.loads((current / "queries.json").read_text()):
        failures.append("resolved query workload differs from baseline")
    if old["code_revision"] == new["code_revision"]:
        failures.append("baseline and current code revisions are identical")
    return failures


def governor() -> str | None:
    path = Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
    return path.read_text().strip() if path.exists() else None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("quick", "full"))
    parser.add_argument("--result-dir", type=Path)
    parser.add_argument("--baseline", type=Path, required=True, help="explicit completed baseline run directory")
    parser.add_argument("--comparison", choices=("repair", "historical", "casefold", "parallel"), required=True)
    parser.add_argument("--verify-only", action="store_true", help="check an existing completed run")
    args = parser.parse_args()
    revision = subprocess.check_output(["git", "rev-parse", "--short=10", "HEAD"], cwd=ROOT, text=True).strip()
    result_dir = (args.result_dir or ROOT / "bench/results/musenalm" / f"{args.mode}-{revision}").resolve()
    result_dir.mkdir(parents=True, exist_ok=True)
    profiles = PROFILES if args.mode == "full" else PROFILES[:1]
    repetitions = 5 if args.mode == "full" else 1
    command = [
        "cargo", "run", "--release", "--example", "bench_musenalm", "--",
        "--profiles", ",".join(profiles), "--repetitions", str(repetitions),
        "--result-dir", str(result_dir),
    ]
    if not args.verify_only:
        with (result_dir / "console.log").open("w") as console:
            completed = subprocess.run(command, cwd=ROOT, stdout=console, stderr=subprocess.STDOUT)
        if completed.returncode:
            print(f"benchmark failed: see {result_dir / 'console.log'}", file=sys.stderr)
            return completed.returncode
    baseline = args.baseline.resolve()
    old_manifest = json.loads((baseline / "run-manifest.json").read_text())
    new_manifest = json.loads((result_dir / "run-manifest.json").read_text())
    old_rows = rows(baseline / "query-results.csv")
    new_rows = rows(result_dir / "query-results.csv")
    jq_counts = json.loads(JQ_COUNTS.read_text())["counts"]
    expected_keys = {(f"Q{index:03}", profile) for index in range(1, 151) for profile in profiles}
    failures = []
    if set(new_rows) != expected_keys:
        failures.append(f"query row keys differ: missing={sorted(expected_keys - set(new_rows))}, extra={sorted(set(new_rows) - expected_keys)}")
    if set(jq_counts) != {f"Q{index:03}" for index in range(1, 151)}:
        failures.append("independent jq count baseline does not cover all 150 cases")
    if new_manifest["repetitions"] != repetitions or new_manifest["failures"] != 0:
        failures.append("benchmark repetition or failure count invalid")
    failures.extend(compatibility_failures(baseline, result_dir, old_manifest, new_manifest))
    if args.comparison == "repair" and old_manifest["code_revision"] != "92cdb92b19200cb0005af23b66aeb3cd1b514c4b":
        failures.append("repair comparison must use the immediate foundation baseline revision")
    if args.comparison == "historical" and old_manifest["code_revision"] != "128a7601c2defec3fd9f5dbb851fb2a4303f7f9a":
        failures.append("historical comparison must use the named historical baseline revision")
    if args.comparison == "casefold" and old_manifest["code_revision"] != "20261604bebf949131441c8262f1f442bce4207c":
        failures.append("casefold comparison must use the current foundation-repair baseline revision")
    if args.comparison == "parallel" and old_manifest["code_revision"] != "ff759b225c6bc4bfdd614c542fe38fe017a900b6":
        failures.append("parallel comparison must use the case-folded query baseline revision")
    build_flags = {key: value for key, value in os.environ.items()
                   if key in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET")
                   or key.startswith("CARGO_PROFILE_RELEASE_")}
    if build_flags:
        failures.append(f"nondefault build flags are not recorded in the pre-repair baseline: {sorted(build_flags)}")
    baseline_environment_path = baseline / "environment.json"
    baseline_environment = json.loads(baseline_environment_path.read_text()) if baseline_environment_path.exists() else {
        "governor": "performance" if args.comparison in ("casefold", "parallel") else "powersave",
        "affinity": list(range(32)), "build_flags": {},
    }
    affinity = sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None
    if baseline_environment["governor"] != governor():
        failures.append("CPU governor differs from declared baseline environment")
    if baseline_environment["affinity"] != affinity:
        failures.append("CPU affinity differs from declared baseline environment")
    samples = [json.loads(line) for line in (result_dir / "measurements.jsonl").read_text().splitlines()]
    if len(samples) != 150 * len(profiles) * repetitions:
        failures.append(f"sample count {len(samples)} differs from {150 * len(profiles) * repetitions}")
    groups: dict[tuple[str, str], list[dict]] = defaultdict(list)
    for sample in samples:
        profile = sample["profile"]
        name = profile["alphabet"] + ("-warm" if profile["warm"] else "-off")
        groups[(sample["case_id"], name)].append(sample)
        if sample["status"] != "ok":
            failures.append(f"{sample['case_id']} {name}: {sample['status']} {sample['error']}")
    for key in expected_keys:
        if len(groups[key]) != repetitions:
            failures.append(f"{key}: expected {repetitions} samples, found {len(groups[key])}")
    for key in expected_keys & set(new_rows):
        old = old_rows[key]
        new = new_rows[key]
        for field in ("status", "total_matches", "returned_records"):
            if old[field] != new[field]:
                failures.append(f"{key}: {field} baseline={old[field]} actual={new[field]}")
        if int(new["total_matches"]) != jq_counts[key[0]]:
            failures.append(f"{key}: count {new['total_matches']} differs from jq {jq_counts[key[0]]}")
    cpu_model = next(
        (line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")),
        "unknown",
    )
    same_machine = cpu_model == "13th Gen Intel(R) Core(TM) i9-13900K" and platform.machine() == "x86_64"
    if not same_machine:
        failures.append("hardware differs from recorded baseline environment")
    relative = 1.5 if args.mode == "full" else 1.8
    absolute_ns = 2_000_000 if args.mode == "full" else 5_000_000
    slowdowns = []
    latency_failures = []
    for key in sorted(expected_keys & set(new_rows)):
        old_ns = int(old_rows[key]["median_ns"])
        new_ns = int(new_rows[key]["median_ns"])
        slowdowns.append((new_ns / max(old_ns, 1), key, old_ns, new_ns))
        if same_machine and new_ns > max(old_ns * relative, old_ns + absolute_ns):
            latency_failures.append((key, old_ns, new_ns))
    confirmations = []
    if args.mode == "quick" and latency_failures:
        confirmation_dir = result_dir / "confirmation"
        confirmation_csv = confirmation_dir / "query-results.csv"
        confirmation_command = [
            "cargo", "run", "--release", "--example", "bench_musenalm", "--",
            "--cases", ",".join(key[0] for key, _, _ in latency_failures),
            "--profiles", "ascii-off", "--repetitions", "3",
            "--result-dir", str(confirmation_dir),
        ]
        if not confirmation_csv.is_file():
            confirmation_dir.mkdir(parents=True, exist_ok=True)
            with (confirmation_dir / "console.log").open("w") as console:
                completed = subprocess.run(confirmation_command, cwd=ROOT, stdout=console, stderr=subprocess.STDOUT)
            if completed.returncode:
                failures.append(f"confirmation benchmark failed: {confirmation_dir / 'console.log'}")
        if confirmation_csv.is_file():
            confirmed_rows = rows(confirmation_csv)
            confirmation_manifest = json.loads((confirmation_dir / "run-manifest.json").read_text())
            _, confirmation_schema_error = schema_identity(
                confirmation_dir,
                {registration["name"] for registration in confirmation_manifest["registrations"]},
                confirmation_manifest.get("schema_manifest_digest"),
            )
            if confirmation_schema_error:
                failures.append(f"confirmation {confirmation_schema_error}")
            confirmation_samples = [
                json.loads(line) for line in (confirmation_dir / "measurements.jsonl").read_text().splitlines()
            ]
            expected_confirmation_keys = {key for key, _, _ in latency_failures}
            if set(confirmed_rows) != expected_confirmation_keys:
                failures.append("confirmation case set differs from flagged cases")
            if len(confirmation_samples) != 3 * len(latency_failures):
                failures.append("confirmation sample count invalid")
            confirmation_groups: dict[tuple[str, str], int] = defaultdict(int)
            if confirmation_manifest["repetitions"] != 3 or confirmation_manifest["failures"] != 0:
                failures.append("confirmation repetition or failure count invalid")
            for field in ("corpus_fingerprint", "schema_fingerprint", "workload_fingerprint", "code_revision", "compiler"):
                if confirmation_manifest[field] != new_manifest[field]:
                    failures.append(f"confirmation {field} differs from main run")
            for sample in confirmation_samples:
                sample_profile = sample["profile"]["alphabet"] + ("-warm" if sample["profile"]["warm"] else "-off")
                confirmation_groups[(sample["case_id"], sample_profile)] += 1
                if (sample["case_id"], sample_profile) not in expected_confirmation_keys or sample["status"] != "ok":
                    failures.append(f"unexpected or failed confirmation sample: {sample['case_id']} {sample_profile}")
            if any(confirmation_groups[key] != 3 for key in expected_confirmation_keys):
                failures.append("confirmation samples per case invalid")
            for key, old_ns, first_ns in latency_failures:
                confirmed = confirmed_rows.get(key)
                if confirmed is None:
                    failures.append(f"{key}: confirmation result missing")
                    continue
                median_ns = int(confirmed["median_ns"])
                threshold_ns = max(old_ns * relative, old_ns + absolute_ns)
                passed = confirmed["status"] == "ok" and int(confirmed["total_matches"]) == jq_counts[key[0]] and median_ns <= threshold_ns
                confirmations.append({"case": key[0], "profile": key[1], "first_ns": first_ns,
                                      "confirmed_median_ns": median_ns, "threshold_ns": threshold_ns,
                                      "passed": passed})
                if not passed:
                    failures.append(f"{key}: confirmed {median_ns / 1e6:.3f} ms exceeds {old_ns / 1e6:.3f} ms baseline")
    else:
        for key, old_ns, new_ns in latency_failures:
            failures.append(f"{key}: {new_ns / 1e6:.3f} ms exceeds {old_ns / 1e6:.3f} ms baseline")
    old_total = sum(int(old_rows[key]["median_ns"]) for key in expected_keys)
    new_total = sum(int(new_rows[key]["median_ns"]) for key in expected_keys & set(new_rows))
    aggregate_limit = 1.25 if args.mode == "full" else 1.35
    if same_machine and new_total > old_total * aggregate_limit:
        failures.append(f"aggregate latency ratio {new_total / old_total:.3f} exceeds {aggregate_limit}")
    result = {
        "mode": args.mode, "expected_samples": 150 * len(profiles) * repetitions,
        "jq_count_baseline": str(JQ_COUNTS),
        "comparison": args.comparison,
        "baseline_code_revision": old_manifest["code_revision"],
        "actual_code_revision": new_manifest["code_revision"],
        "build_flags": build_flags,
        "baseline_environment": baseline_environment,
        "governor": governor(),
        "path_sensitive_fingerprint_difference": {
            "schema": old_manifest["schema_fingerprint"] != new_manifest["schema_fingerprint"],
            "workload": old_manifest["workload_fingerprint"] != new_manifest["workload_fingerprint"],
        },
        "cpu_model": cpu_model,
        "architecture": platform.machine(),
        "affinity": affinity,
        "actual_samples": len(samples), "baseline": str(baseline),
        "result_dir": str(result_dir), "same_machine": same_machine,
        "aggregate_latency_ratio": new_total / old_total,
        "thresholds": {"per_case_relative": relative, "per_case_absolute_ns": absolute_ns, "aggregate_relative": aggregate_limit},
        "confirmations": confirmations,
        "worst_slowdowns": [
            {"ratio": ratio, "case": key[0], "profile": key[1], "baseline_ns": old_ns, "actual_ns": new_ns}
            for ratio, key, old_ns, new_ns in sorted(slowdowns, reverse=True)[:20]
        ],
        "failures": failures,
        "command": command,
    }
    (result_dir / "gate.json").write_text(json.dumps(result, indent=2) + "\n")
    queries = {case["id"]: case for case in json.loads((result_dir / "queries.json").read_text())}
    comparison_rows = []
    for case, profile in sorted(expected_keys & set(new_rows)):
        previous = old_rows[(case, profile)]
        current = new_rows[(case, profile)]
        old_ns = int(previous["median_ns"])
        new_ns = int(current["median_ns"])
        comparison_rows.append({
            "case_id": case, "profile": profile, "baseline_median_ms": old_ns / 1e6,
            "query": json.dumps(queries[case], ensure_ascii=False, separators=(",", ":")),
            "current_min_ms": int(current["min_ns"]) / 1e6,
            "current_median_ms": new_ns / 1e6,
            "current_max_ms": int(current["max_ns"]) / 1e6,
            "difference_ms": (new_ns - old_ns) / 1e6,
            "difference_percent": (new_ns / max(old_ns, 1) - 1) * 100,
            "samples": current["samples"], "matches": current["total_matches"],
            "page": current["returned_records"], "status": current["status"],
            "baseline_candidates": previous["candidates"], "candidates": current["candidates"],
            "baseline_files_opened": previous["files_opened"], "files_opened": current["files_opened"],
            "scanned_records": current["scanned_records"], "record_parses": current["record_parses"],
        })
    with (result_dir / "comparison.csv").open("w", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=list(comparison_rows[0]))
        writer.writeheader()
        writer.writerows(comparison_rows)
    table = [
        f"# {args.comparison} query timing comparison", "",
        f"Baseline: `{old_manifest['code_revision']}` at `{baseline}`. Current: `{new_manifest['code_revision']}`.",
        f"Profiles: {', '.join(profiles)}. Repetitions per case/profile: {repetitions}.", "",
        "| Case | Profile | Query and options | Before ms | Min ms | Median ms | Max ms | Δ ms | Δ % | Candidates before→after | Files opened before→after | Scanned | Parses | Matches | Page | Status |",
        "|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|",
    ]
    for row in comparison_rows:
        table.append(
            f"| {row['case_id']} | {row['profile']} | `{row['query'].replace('|', '&#124;')}` | {row['baseline_median_ms']:.3f} | "
            f"{row['current_min_ms']:.3f} | {row['current_median_ms']:.3f} | "
            f"{row['current_max_ms']:.3f} | {row['difference_ms']:+.3f} | "
            f"{row['difference_percent']:+.1f} | {row['baseline_candidates']}→{row['candidates']} | "
            f"{row['baseline_files_opened']}→{row['files_opened']} | {row['scanned_records']} | {row['record_parses']} | {row['matches']} | "
            f"{row['page']} | {row['status']} |"
        )
    table_text = "\n".join(table) + "\n"
    (result_dir / "TIMING_TABLE.md").write_text(table_text)
    print(table_text, end="")
    print(f"{args.mode}: {len(samples)} samples, aggregate ratio {new_total / old_total:.3f}, failures {len(failures)}")
    print(f"Full table: {result_dir / 'TIMING_TABLE.md'}")
    for failure in failures[:20]:
        print(failure, file=sys.stderr)
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
