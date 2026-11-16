#!/usr/bin/env python3
"""Run the matched native/stable-index construction campaign.

The driver keeps raw JSON outside versioned result directories by default and
writes the required comparison metadata into the selected result directory.
It never runs the native executable from the candidate target directory.
"""

import argparse
import json
import pathlib
import statistics
import subprocess
import time


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=pathlib.Path, required=True)
    parser.add_argument("--baseline-exe", type=pathlib.Path, required=True)
    parser.add_argument("--candidate-exe", type=pathlib.Path, required=True)
    parser.add_argument("--original-exe", type=pathlib.Path)
    parser.add_argument("--result-dir", type=pathlib.Path, required=True)
    parser.add_argument("--workers", type=int, default=8)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--alphabet", default="ascii", choices=["ascii", "unicode"])
    parser.add_argument("--synthetic-root", type=pathlib.Path)
    parser.add_argument("--baseline-fixture-exe", type=pathlib.Path)
    parser.add_argument("--candidate-fixture-exe", type=pathlib.Path)
    parser.add_argument("--primitives-snapshot", type=pathlib.Path)
    parser.add_argument("--primitives-repetitions", type=int, default=200)
    parser.add_argument("--primitives-extended-relations", action="store_true")
    return parser.parse_args()


def run(command, cwd=None):
    started = time.time_ns()
    completed = subprocess.run(
        [str(part) for part in command],
        cwd=cwd,
        check=False,
        capture_output=True,
        text=True,
    )
    elapsed = time.time_ns() - started
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed ({completed.returncode}) after {elapsed} ns: "
            f"{' '.join(str(part) for part in command)}\n"
            f"{completed.stdout}\n{completed.stderr}"
        )
    return completed.stdout


def read_json(path):
    return json.loads(path.read_text())


def main():
    args = parse_args()
    args.result_dir.mkdir(parents=True, exist_ok=True)
    baseline_runs = []
    original_runs = []
    candidate_runs = []
    for sample in range(1, args.samples + 1):
        baseline_path = args.result_dir / f"baseline-{sample}.json"
        candidate_dir = args.result_dir / f"candidate-{sample}"
        candidate_dir.mkdir(exist_ok=True)
        baseline_json = json.loads(
            run(
                [
                    args.baseline_exe,
                    "--root",
                    args.root,
                    "--alphabet",
                    args.alphabet,
                    "--build-workers",
                    args.workers,
                ]
            )
        )
        baseline_path.write_text(json.dumps(baseline_json, indent=2) + "\n")
        baseline_runs.append(baseline_json)
        if args.original_exe:
            original_path = args.result_dir / f"original-{sample}.json"
            original_json = json.loads(
                run(
                    [
                        args.original_exe,
                        "--mode",
                        "source-build",
                        "--construction",
                        "direct",
                        "--root",
                        args.root,
                        "--alphabet",
                        args.alphabet,
                        "--build-workers",
                        args.workers,
                    ]
                )
            )
            original_path.write_text(json.dumps(original_json, indent=2) + "\n")
            original_runs.append(original_json.get("build", original_json))
        run(
            [
                args.candidate_exe,
                "--mode",
                "source-build",
                "--construction",
                "direct",
                "--root",
                args.root,
                "--alphabet",
                args.alphabet,
                "--build-workers",
                args.workers,
                "--result-dir",
                candidate_dir,
            ]
        )
        candidate_runs.append(read_json(candidate_dir / "build.json"))

    baseline_ns = [run["build_ns"] for run in baseline_runs]
    candidate_ns = [run["total_source_build_ns"] for run in candidate_runs]
    baseline_memory = baseline_runs[-1]
    candidate_memory = read_json(
        args.result_dir / f"candidate-{args.samples}" / "memory.json"
    )
    original_ns = [run["total_source_build_ns"] for run in original_runs]
    comparison = {
        "task": "ic-stable-index-prototype",
        "workers": args.workers,
        "alphabet": args.alphabet,
        "samples": args.samples,
        "native_build_ns": {
            "median": statistics.median(baseline_ns),
            "min": min(baseline_ns),
            "max": max(baseline_ns),
        },
        "candidate_total_build_ns": {
            "median": statistics.median(candidate_ns),
            "min": min(candidate_ns),
            "max": max(candidate_ns),
        },
        "original_build_ns": {
            "median": statistics.median(original_ns) if original_ns else None,
            "min": min(original_ns) if original_ns else None,
            "max": max(original_ns) if original_ns else None,
        },
        "median_ratio": statistics.median(candidate_ns)
        / statistics.median(baseline_ns),
        "original_ratio": (
            statistics.median(candidate_ns) / statistics.median(original_ns)
            if original_ns
            else None
        ),
        "improvement_over_original": (
            1.0 - statistics.median(candidate_ns) / statistics.median(original_ns)
            if original_ns
            else None
        ),
        "memory": {
            "native_resident_bytes": baseline_memory.get("resident_bytes"),
            "native_peak_bytes": baseline_memory.get("peak_resident_bytes"),
            "candidate_resident_bytes": candidate_memory.get(
                "readiness_rss_bytes"
            ),
            "candidate_peak_bytes": candidate_memory.get("readiness_hwm_bytes"),
            "candidate_owned_component_payload_bytes": candidate_memory.get(
                "owned_component_payload_bytes"
            ),
        },
        "candidate_stage_ns": candidate_runs[-1].get("source_build_timings"),
        "candidate_components": candidate_runs[-1].get("components"),
        "field_job_overlap_pairs": read_json(
            args.result_dir / f"candidate-{args.samples}" / "components.json"
        ).get("overlap_pairs"),
    }
    native_resident = comparison["memory"]["native_resident_bytes"]
    native_peak = comparison["memory"]["native_peak_bytes"]
    if native_resident and comparison["memory"]["candidate_resident_bytes"]:
        comparison["memory"]["resident_ratio"] = (
            comparison["memory"]["candidate_resident_bytes"] / native_resident
        )
    if native_peak and comparison["memory"]["candidate_peak_bytes"]:
        comparison["memory"]["peak_ratio"] = (
            comparison["memory"]["candidate_peak_bytes"] / native_peak
        )
    comparison["gate"] = {
        "classification": "performance_rejection"
        if comparison["median_ratio"] > 1.10
        else "provisional_near_parity",
        "source_build_target": "<= 1.10x native",
        "resident_rss_target": "<= 1.40x native",
        "peak_rss_target": "<= 1.60x native",
    }

    if args.synthetic_root:
        if not args.baseline_fixture_exe or not args.candidate_fixture_exe:
            raise SystemExit(
                "synthetic runs need --baseline-fixture-exe and "
                "--candidate-fixture-exe"
            )
        synthetic = {}
        for workers in (1, 2, args.workers):
            baseline_dir = args.result_dir / f"synthetic-baseline-p{workers}"
            stable_dir = args.result_dir / f"synthetic-stable-p{workers}"
            baseline_dir.mkdir(exist_ok=True)
            stable_dir.mkdir(exist_ok=True)
            run(
                [
                    args.baseline_fixture_exe,
                    "--root",
                    args.synthetic_root,
                    "--implementation",
                    "native",
                    "--build-workers",
                    workers,
                    "--result-dir",
                    baseline_dir,
                ]
            )
            run(
                [
                    args.candidate_fixture_exe,
                    "--root",
                    args.synthetic_root,
                    "--implementation",
                    "stable",
                    "--build-workers",
                    workers,
                    "--result-dir",
                    stable_dir,
                ]
            )
            baseline = read_json(baseline_dir / "build.json")
            stable = read_json(stable_dir / "build.json")
            synthetic[str(workers)] = {
                "native_build_ns": baseline["total_source_build_ns"],
                "stable_build_ns": stable["total_source_build_ns"],
                "ratio": stable["total_source_build_ns"]
                / baseline["total_source_build_ns"],
                "native_resident_bytes": baseline.get("resident_bytes"),
                "native_peak_bytes": baseline.get("peak_resident_bytes"),
                "stable_resident_bytes": stable.get("resident_bytes"),
                "stable_peak_bytes": stable.get("peak_resident_bytes"),
                "stable_components": stable.get("components"),
                "field_job_overlap_pairs": stable.get("field_job_overlap_pairs"),
            }
        comparison["synthetic"] = synthetic

    if args.primitives_snapshot:
        plan_path = args.result_dir / "primitives-plan.json"
        plan_command = [
            args.candidate_exe,
            "--mode",
            "primitive-plan",
            "--snapshot-input",
            args.primitives_snapshot,
            "--queries-output",
            plan_path,
        ]
        run(plan_command)
        native_dir = args.result_dir / "primitives-native"
        stable_dir = args.result_dir / "primitives-stable"
        native_dir.mkdir(exist_ok=True)
        stable_dir.mkdir(exist_ok=True)
        relation_flag = (
            ["--extended-relations"]
            if args.primitives_extended_relations
            else []
        )
        run(
            [
                args.candidate_exe,
                "--mode",
                "native-primitives",
                *relation_flag,
                "--root",
                args.root,
                "--queries",
                plan_path,
                "--alphabet",
                args.alphabet,
                "--build-workers",
                args.workers,
                "--repetitions",
                args.primitives_repetitions,
                "--result-dir",
                native_dir,
            ]
        )
        run(
            [
                args.candidate_exe,
                "--mode",
                "primitives",
                "--snapshot-input",
                args.primitives_snapshot,
                "--queries",
                plan_path,
                "--alphabet",
                args.alphabet,
                "--repetitions",
                args.primitives_repetitions,
                "--result-dir",
                stable_dir,
            ]
        )
        native_primitives = read_json(native_dir / "primitives.json")
        stable_primitives = read_json(stable_dir / "primitives.json")
        native_rows = native_primitives["resident_primitives"]
        stable_rows = stable_primitives["resident_primitives"]
        mismatches = []
        for index, (native, stable) in enumerate(zip(native_rows, stable_rows)):
            if (
                native["family"] != stable["family"]
                or native["result_members"] != stable["result_members"]
            ):
                mismatches.append(
                    {
                        "row": index,
                        "native_family": native["family"],
                        "stable_family": stable["family"],
                        "native_members": native["result_members"],
                        "stable_members": stable["result_members"],
                        "query": native.get("query"),
                    }
                )
        families = {}
        for native, stable in zip(native_rows, stable_rows):
            family = native["family"]
            entry = families.setdefault(
                family,
                {
                    "rows": 0,
                    "native_median_ns": [],
                    "stable_median_ns": [],
                    "stable_p95_ns": [],
                },
            )
            entry["rows"] += 1
            entry["native_median_ns"].append(native["median_ns"])
            entry["stable_median_ns"].append(stable["median_ns"])
            entry["stable_p95_ns"].append(stable["p95_ns"])
        for entry in families.values():
            native_median = statistics.median(entry.pop("native_median_ns"))
            stable_median = statistics.median(entry.pop("stable_median_ns"))
            entry["native_median_of_medians_ns"] = native_median
            entry["stable_median_of_medians_ns"] = stable_median
            entry["median_ratio"] = (
                stable_median / native_median if native_median else None
            )
            entry["stable_p95_max_ns"] = max(entry.pop("stable_p95_ns"))
        primitives = {
            "task": "ic-stable-index-prototype-repair",
            "snapshot": str(args.primitives_snapshot),
            "alphabet": args.alphabet,
            "repetitions": args.primitives_repetitions,
            "rows": len(native_rows),
            "result_mismatch_count": len(mismatches),
            "result_mismatches": mismatches[:20],
            "families": families,
            "native": native_primitives,
            "stable": stable_primitives,
        }
        (args.result_dir / "primitives.json").write_text(
            json.dumps(primitives, indent=2) + "\n"
        )
        comparison["primitives"] = {
            "rows": len(native_rows),
            "result_mismatch_count": len(mismatches),
            "families": families,
            "output": str(args.result_dir / "primitives.json"),
        }

    (args.result_dir / "comparison.json").write_text(
        json.dumps(comparison, indent=2) + "\n"
    )
    print(json.dumps(comparison, indent=2))


if __name__ == "__main__":
    main()
