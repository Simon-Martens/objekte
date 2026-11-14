#!/usr/bin/env python3
"""Summarize archived paired query CSVs without discarding samples."""

import csv
import json
import math
import statistics
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1] / "raw"


def rows(path):
    with path.open(newline="") as stream:
        return {
            (row["case_id"], row["profile"]): row
            for row in csv.DictReader(stream)
        }


def summarize(campaign, repetitions):
    folder = ROOT / campaign
    all_ratios = {}
    pair_geomeans = []
    digest_parity = True
    for pair in range(3):
        baseline = rows(folder / f"{pair}-base" / "query-results.csv")
        candidate = rows(folder / f"{pair}-dense" / "query-results.csv")
        assert baseline.keys() == candidate.keys()
        assert all(row["status"] == "ok" for row in (*baseline.values(), *candidate.values()))
        ratios = []
        for key in baseline:
            left = int(baseline[key]["median_ns"])
            right = int(candidate[key]["median_ns"])
            ratios.append(right / left)
            all_ratios.setdefault(key, []).append(right / left)
            assert int(baseline[key]["samples"]) == repetitions
            assert int(candidate[key]["samples"]) == repetitions
            assert int(baseline[key]["verified_result_hits"]) == 0
            assert int(candidate[key]["verified_result_hits"]) == 0
        pair_geomeans.append(math.exp(statistics.mean(map(math.log, ratios))))
        for label in ("base", "dense"):
            manifest = json.loads((folder / f"{pair}-{label}" / "run-manifest.json").read_text())
            assert manifest["failures"] == 0
        left_digests = json.loads((folder / f"{pair}-base" / "result-digests.json").read_text())
        right_digests = json.loads((folder / f"{pair}-dense" / "result-digests.json").read_text())
        digest_parity &= left_digests == right_digests
    median_ratios = {f"{case}/{profile}": statistics.median(values) for (case, profile), values in all_ratios.items()}
    values = list(median_ratios.values())
    return {
        "campaign": campaign,
        "pairs": 3,
        "case_profiles": len(values),
        "repetitions_per_case_profile_per_run": repetitions,
        "digest_parity": digest_parity,
        "pair_geomeans": pair_geomeans,
        "geomean_per_case_median_ratio": math.exp(statistics.mean(map(math.log, values))),
        "median_ratios": median_ratios,
    }


if __name__ == "__main__":
    output = [summarize("paired-full5", 5), summarize("paired-tail", 30)]
    print(json.dumps(output, indent=2, sort_keys=True))
