#!/usr/bin/env python3
"""Recalculate the final hybrid-store comparison from archived raw samples."""

import gzip
import json
import math
import statistics
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1] / "final-e15b"
MODES = ("baseline", "memory", "resident", "ondemand")
TAIL_CASES = ("Q001", "Q008", "Q042", "Q143", "Q167", "Q175", "Q177", "Q194", "Q202")


def middle_pair_median(values):
    ordered = sorted(values)
    middle = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[middle]
    return (ordered[middle - 1] + ordered[middle] + 1) // 2


def nearest_rank_p95(values):
    ordered = sorted(values)
    return ordered[math.ceil(0.95 * len(ordered)) - 1]


def geometric_mean(values):
    return math.exp(statistics.mean(math.log(value) for value in values))


def query_samples(mode):
    folder = ROOT / "queries" / mode
    rows = defaultdict(list)
    with gzip.open(folder / "measurements.jsonl.gz", "rt") as stream:
        for line in stream:
            sample = json.loads(line)
            if sample["status"] != "ok" or sample["counters"]["verified_result_hit"]:
                raise ValueError(f"{mode}: failed query or verified-result cache hit")
            key = (sample["case_id"], sample["profile"]["alphabet"])
            rows[key].append(sample["total_ns"])
    if len(rows) != 440 or any(len(values) != 30 for values in rows.values()):
        raise ValueError(f"{mode}: expected 440 case/profile pairs of 30 samples")
    return {
        key: {
            "median_ns": middle_pair_median(values),
            "p95_ns": nearest_rank_p95(values),
        }
        for key, values in rows.items()
    }


def paired_stats(kind, measure, names):
    rows = json.loads((ROOT / "paired" / kind / "trials.json").read_text())["records"]
    by_name = {}
    for name in names:
        selected = [row for row in rows if row.get("label", row.get("mode")) == name]
        if len(selected) != 7:
            raise ValueError(f"{kind}/{name}: expected seven samples")
        by_name[name] = {
            "median": middle_pair_median([row[measure] for row in selected]),
            "samples": [row[measure] for row in selected],
        }
        for extra in ("first_batch_ns", "after_build_rss", "after_build_hwm", "stored_bytes"):
            if extra in selected[0]:
                by_name[name][extra + "_median"] = middle_pair_median([row[extra] for row in selected])
    baseline = by_name[names[0]]["median"]
    return {"measure": measure, "roles": by_name,
            "ratios_to_baseline": {name: by_name[name]["median"] / baseline for name in names[1:]}}


def main():
    digests = [json.loads((ROOT / "queries" / mode / "result-digests.json").read_text()) for mode in MODES]
    if any(value != digests[0] for value in digests[1:]):
        raise ValueError("result digests differ")
    manifests = [json.loads((ROOT / "queries" / mode / "run-manifest.json").read_text()) for mode in MODES]
    for key in ("corpus_fingerprint", "schema_fingerprint", "workload_fingerprint"):
        if any(value[key] != manifests[0][key] for value in manifests[1:]):
            raise ValueError(f"{key} differs")
    samples = {mode: query_samples(mode) for mode in MODES}
    baseline = samples["baseline"]
    query = {}
    for mode in MODES[1:]:
        current = samples[mode]
        query[mode] = {
            "median_geomean_ratio": geometric_mean(current[key]["median_ns"] / baseline[key]["median_ns"] for key in baseline),
            "p95_geomean_ratio": geometric_mean(current[key]["p95_ns"] / baseline[key]["p95_ns"] for key in baseline),
            "tail_cases": {
                case: {
                    alphabet: {
                        "baseline": baseline[(case, alphabet)],
                        "candidate": current[(case, alphabet)],
                        "median_ratio": current[(case, alphabet)]["median_ns"] / baseline[(case, alphabet)]["median_ns"],
                        "p95_ratio": current[(case, alphabet)]["p95_ns"] / baseline[(case, alphabet)]["p95_ns"],
                    }
                    for alphabet in ("ascii", "unicode")
                }
                for case in TAIL_CASES
            },
        }
    result = {
        "statistics": {
            "median": "middle pair mean, rounded to nearest integer nanosecond with half up",
            "p95": "nearest rank: sorted[ceil(0.95 * n) - 1]",
            "geometric_mean": "exp(mean(ln(candidate/baseline))) over 440 case/profile pairs",
        },
        "digest_parity": True,
        "case_profiles": 440,
        "repetitions_per_case_profile": 30,
        "builds": paired_stats("builds", "build_ns", ("baseline", "candidate")),
        "opens": paired_stats("opens", "open_ns", ("baseline", "ondemand", "resident")),
        "flushes": paired_stats("flushes", "flush_ns", ("baseline", "candidate")),
        "queries": query,
    }
    (ROOT / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: value["median_geomean_ratio"] for key, value in query.items()}, indent=2))


if __name__ == "__main__":
    main()
