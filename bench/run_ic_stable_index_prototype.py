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
    candidate_memories = []
    candidate_stages = []
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
        candidate_build = read_json(candidate_dir / "build.json")
        candidate_runs.append(candidate_build)
        candidate_memories.append(read_json(candidate_dir / "memory.json"))
        candidate_stages.append(candidate_build.get("source_build_timings") or {})

    def stats(values):
        values = [value for value in values if value is not None]
        if not values:
            return None
        return {
            "median": statistics.median(values),
            "min": min(values),
            "max": max(values),
            "samples": len(values),
        }

    baseline_ns = [run["build_ns"] for run in baseline_runs]
    candidate_ns = [run["total_source_build_ns"] for run in candidate_runs]
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
            "native_resident_bytes": stats([run.get("resident_bytes") for run in baseline_runs]),
            "native_peak_bytes": stats([run.get("peak_resident_bytes") for run in baseline_runs]),
            "candidate_resident_bytes": stats([run.get("readiness_rss_bytes") for run in candidate_memories]),
            "candidate_peak_bytes": stats([run.get("readiness_hwm_bytes") for run in candidate_memories]),
            "candidate_owned_component_payload_bytes": stats([
                run.get("owned_component_payload_bytes") for run in candidate_memories
            ]),
        },
        "candidate_stage_ns": {
            key: stats([stage.get(key) for stage in candidate_stages])
            for key in (
                "discovery_ns",
                "setup_ns",
                "extraction_admission_ns",
                "finalization_ns",
                "stable_component_ns",
            )
        },
        "candidate_components": stats([run.get("components") for run in candidate_runs]),
        "field_job_overlap_pairs": stats([
            read_json(args.result_dir / f"candidate-{sample}" / "components.json").get("overlap_pairs")
            for sample in range(1, args.samples + 1)
        ]),
        "selected_trace": {
            "sample": args.samples,
            "note": "component trace is one run, not a median",
        },
    }
    native_resident = comparison["memory"]["native_resident_bytes"]
    native_peak = comparison["memory"]["native_peak_bytes"]
    if native_resident and comparison["memory"]["candidate_resident_bytes"]:
        comparison["memory"]["resident_ratio"] = (
            comparison["memory"]["candidate_resident_bytes"]["median"]
            / native_resident["median"]
        )
    if native_peak and comparison["memory"]["candidate_peak_bytes"]:
        comparison["memory"]["peak_ratio"] = (
            comparison["memory"]["candidate_peak_bytes"]["median"]
            / native_peak["median"]
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

    (args.result_dir / "comparison.json").write_text(
        json.dumps(comparison, indent=2) + "\n"
    )
    print(json.dumps(comparison, indent=2))


if __name__ == "__main__":
    main()
