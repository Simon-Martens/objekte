#!/usr/bin/env python3
"""Run the preselected tail suite on matching incremental backends."""

import argparse
import hashlib
import json
import math
import subprocess
from pathlib import Path


BASELINE_REVISION = "da8fde68472b8646a63a05b3b3654c58de877301"
CASES = ("Q001", "Q008", "Q042", "Q143", "Q167", "Q175", "Q177", "Q194", "Q202")
PROFILES = ("ascii-off", "unicode-off")
REPETITIONS = 30


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(*args: str) -> str:
    return subprocess.check_output(("git", *args), text=True).strip()


def median_ns(samples: list[int]) -> int:
    ordered = sorted(samples)
    middle = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[middle]
    left, right = ordered[middle - 1 : middle + 1]
    return left + (right - left + 1) // 2


def p95_ns(samples: list[int]) -> int:
    ordered = sorted(samples)
    return ordered[math.ceil(0.95 * len(ordered)) - 1]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--candidate-build-revision", required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--store-base", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    if not args.store_base.is_dir() or not any(args.store_base.iterdir()):
        raise RuntimeError("--store-base must name a prebuilt managed store base")
    runner_revision = git("rev-parse", "HEAD")
    if git("status", "--porcelain"):
        raise RuntimeError("candidate checkout must be clean for provenance")
    lock_bytes = subprocess.check_output(("git", "show", f"{BASELINE_REVISION}:Cargo.lock"))
    provenance = {
        "baseline_build_revision": BASELINE_REVISION,
        "candidate_build_revision": args.candidate_build_revision,
        "runner_checkout_revision": runner_revision,
        "baseline_executable_sha256": sha256(args.baseline),
        "candidate_executable_sha256": sha256(args.candidate),
        "baseline_lock_sha256": hashlib.sha256(lock_bytes).hexdigest(),
        "candidate_lock_sha256": sha256(Path("Cargo.lock")),
        "compiler": subprocess.check_output(("rustc", "--version"), text=True).strip(),
        "cases_preselected": CASES,
        "profiles": PROFILES,
        "repetitions": REPETITIONS,
        "median_method": "middle pair mean, rounded to nearest nanosecond with half up",
        "p95_method": "nearest rank: sorted[ceil(0.95 * n) - 1]",
        "warmup": "benchmark default; every repetition measured, no discarded samples",
    }
    manifests = {}
    measurements = {}
    digest_baseline = None
    for mode in ("baseline", "memory", "resident", "ondemand"):
        folder = args.output / mode
        program = args.baseline if mode == "baseline" else args.candidate
        command = [
            str(program), "--root", str(args.root), "--result-dir", str(folder),
            "--extended", "--incremental", "--profiles", ",".join(PROFILES),
            "--cases", ",".join(CASES), "--repetitions", str(REPETITIONS),
            "--verified-cache-bytes", "0", "--prepared-cache-bytes", "16777216",
            "--query-workers", "32",
        ]
        if mode == "memory":
            command.extend(("--storage", "memory"))
        elif mode in ("resident", "ondemand"):
            command.extend(("--storage", "custom", str(args.store_base)))
            if mode == "resident":
                command.append("--fully-resident")
        completed = subprocess.run(command, text=True, capture_output=True)
        folder.mkdir(exist_ok=True)
        (folder / "command.json").write_text(json.dumps(command, indent=2) + "\n")
        (folder / "stdout.log").write_text(completed.stdout)
        (folder / "stderr.log").write_text(completed.stderr)
        if completed.returncode:
            raise RuntimeError(f"{mode}: {completed.stderr[-2000:]}")
        manifest = json.loads((folder / "run-manifest.json").read_text())
        expected_mode = "incremental" if mode == "baseline" else "hybrid"
        if manifest["build_mode"] != expected_mode:
            raise RuntimeError(f"{mode}: backend mismatch")
        if mode != "baseline" and manifest.get("binary_build_revision") != args.candidate_build_revision:
            raise RuntimeError(f"{mode}: candidate binary revision mismatch")
        manifests[mode] = manifest
        if mode in ("resident", "ondemand"):
            index_results = json.loads((folder / "index-results.json").read_text())
            if any(result["source_reads"] != 0 or result["build_parses"] != 0 for result in index_results):
                raise RuntimeError(f"{mode}: store was rebuilt instead of reopened")
        digests = json.loads((folder / "result-digests.json").read_text())
        if digest_baseline is None:
            digest_baseline = digests
        elif digests != digest_baseline:
            raise RuntimeError(f"{mode}: result digest mismatch")
        samples = [json.loads(line) for line in (folder / "measurements.jsonl").read_text().splitlines()]
        if len(samples) != len(CASES) * len(PROFILES) * REPETITIONS:
            raise RuntimeError(f"{mode}: sample count mismatch")
        if any(row["status"] != "ok" or row["counters"]["verified_result_hit"] for row in samples):
            raise RuntimeError(f"{mode}: failed query or result-cache hit")
        measurements[mode] = samples
        print(f"{mode}: {len(samples)} valid samples", flush=True)
    for mode in ("memory", "resident", "ondemand"):
        for field in ("schema_fingerprint", "workload_fingerprint", "corpus_fingerprint"):
            if manifests[mode][field] != manifests["baseline"][field]:
                raise RuntimeError(f"{mode}: {field} mismatch")
    comparison = []
    for case in CASES:
        for alphabet in ("ascii", "unicode"):
            row = {"case": case, "alphabet": alphabet}
            for mode in measurements:
                selected = [
                    sample for sample in measurements[mode]
                    if sample["case_id"] == case and sample["profile"]["alphabet"] == alphabet
                ]
                times = [sample["total_ns"] for sample in selected]
                row[mode] = {
                    "median_ns": median_ns(times),
                    "p95_ns": p95_ns(times),
                    "prepared_plan_hits": sum(sample["counters"]["prepared_plan_hit"] for sample in selected),
                    "verified_result_hits": 0,
                }
            comparison.append(row)
    provenance["commands"] = {mode: json.loads((args.output / mode / "command.json").read_text()) for mode in manifests}
    provenance["corpus_fingerprint"] = manifests["baseline"]["corpus_fingerprint"]
    provenance["workload_fingerprint"] = manifests["baseline"]["workload_fingerprint"]
    provenance["schema_fingerprint"] = manifests["baseline"]["schema_fingerprint"]
    provenance["comparison"] = comparison
    (args.output / "comparison.json").write_text(json.dumps(provenance, indent=2) + "\n")


if __name__ == "__main__":
    main()
