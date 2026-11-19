#!/usr/bin/env python3
"""Compare two query runs without discarding ordered hit identities."""

from __future__ import annotations

import argparse
import hashlib
import json
from collections import Counter
from pathlib import Path

from run_query_regressions import compatibility_failures


def load_json(path: Path):
    return json.loads(path.read_text())


def profile_name(profile: dict) -> str:
    return profile["alphabet"] + ("-warm" if profile["warm"] else "-off")


def sample_key(sample: dict) -> tuple[str, str, int]:
    return sample["case_id"], profile_name(sample["profile"]), sample["repetition"]


def load_samples(path: Path) -> dict[tuple[str, str, int], dict]:
    result = {}
    for line in (path / "measurements.jsonl").read_text().splitlines():
        sample = json.loads(line)
        key = sample_key(sample)
        if key in result:
            raise ValueError(f"duplicate sample in {path}: {key}")
        result[key] = sample
    return result


def first_difference(baseline: dict, current: dict):
    if isinstance(baseline, dict) and isinstance(current, dict):
        for key in sorted(set(baseline) | set(current)):
            if key not in baseline:
                return f"current-only field {key}={current[key]!r}"
            if key not in current:
                return f"baseline-only field {key}={baseline[key]!r}"
            if baseline[key] != current[key]:
                nested = first_difference(baseline[key], current[key])
                return f"{key}: {nested}"
        return "values differ"
    return f"baseline={baseline!r} current={current!r}"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--current", type=Path, required=True)
    parser.add_argument("--baseline-workload", type=Path, required=True)
    parser.add_argument("--current-workload", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    baseline = args.baseline.resolve()
    current = args.current.resolve()
    old_manifest = load_json(baseline / "run-manifest.json")
    new_manifest = load_json(current / "run-manifest.json")
    old_samples = load_samples(baseline)
    new_samples = load_samples(current)
    old_digests = load_json(baseline / "result-digests.json")
    new_digests = load_json(current / "result-digests.json")

    category_counts: Counter[str] = Counter()
    first_differences: dict[str, object] = {}

    def mismatch(category: str, key, detail: str):
        category_counts[category] += 1
        first_differences.setdefault(category, {"key": key, "detail": detail})

    for key in sorted(set(old_samples) | set(new_samples)):
        old = old_samples.get(key)
        new = new_samples.get(key)
        if old is None or new is None:
            mismatch("sample_keys", key, "sample is missing from one side")
            continue
        for category, field in (
            ("status", "status"),
            ("error", "error"),
            ("counters", "counters"),
        ):
            if old[field] != new[field]:
                mismatch(category, key, first_difference(old[field], new[field]))

    for case_id in sorted(set(old_digests) | set(new_digests)):
        if old_digests.get(case_id) != new_digests.get(case_id):
            mismatch(
                "ordered_results",
                case_id,
                f"baseline={old_digests.get(case_id)!r} current={new_digests.get(case_id)!r}",
            )

    queries_equal = load_json(baseline / "queries.json") == load_json(current / "queries.json")
    schemas_equal = load_json(baseline / "schema-manifest.json") == load_json(
        current / "schema-manifest.json"
    )
    old_workload = args.baseline_workload.resolve()
    new_workload = args.current_workload.resolve()
    workload_bytes_equal = old_workload.read_bytes() == new_workload.read_bytes()
    compatibility = compatibility_failures(baseline, current, old_manifest, new_manifest)
    empty_cases = sum(
        samples["counters"]["total_matches"] == 0
        for (case_id, _, repetition), samples in old_samples.items()
        if repetition == 0
    )

    result = {
        "baseline": {
            "path": str(baseline),
            "code_revision": old_manifest["code_revision"],
            "binary_build_revision": old_manifest["binary_build_revision"],
            "executable_sha256": old_manifest["executable_sha256"],
            "workload_path": str(old_workload),
            "workload_sha256": hashlib.sha256(old_workload.read_bytes()).hexdigest(),
            "workload_fingerprint": old_manifest["workload_fingerprint"],
        },
        "current": {
            "path": str(current),
            "code_revision": new_manifest["code_revision"],
            "binary_build_revision": new_manifest["binary_build_revision"],
            "executable_sha256": new_manifest["executable_sha256"],
            "workload_path": str(new_workload),
            "workload_sha256": hashlib.sha256(new_workload.read_bytes()).hexdigest(),
            "workload_fingerprint": new_manifest["workload_fingerprint"],
        },
        "common": {
            "root": old_manifest["root"],
            "corpus_fingerprint": old_manifest["corpus_fingerprint"],
            "source_files": sum(
                registration["files"] for registration in old_manifest["registrations"]
            ),
            "schema_fingerprint": old_manifest["schema_fingerprint"],
            "compiler": old_manifest["compiler"],
            "query_workers": old_manifest["query_workers"],
        },
        "comparison": {
            "baseline_samples": len(old_samples),
            "current_samples": len(new_samples),
            "matched_samples": len(set(old_samples) & set(new_samples)),
            "ordered_result_cases": len(old_digests),
            "empty_cases": empty_cases,
            "nonempty_cases": len(old_digests) - empty_cases,
            "ordered_result_matches": sum(
                old_digests.get(case_id) == new_digests.get(case_id)
                for case_id in set(old_digests) | set(new_digests)
            ),
            "queries_equal": queries_equal,
            "schemas_equal": schemas_equal,
            "workload_bytes_equal": workload_bytes_equal,
            "compatibility_failures": compatibility,
            "mismatch_counts": dict(sorted(category_counts.items())),
            "first_differences": first_differences,
        },
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    failures = bool(category_counts or compatibility)
    failures |= not (queries_equal and schemas_equal and workload_bytes_equal)
    return int(failures)


if __name__ == "__main__":
    raise SystemExit(main())
