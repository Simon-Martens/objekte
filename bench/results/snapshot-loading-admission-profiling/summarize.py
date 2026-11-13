#!/usr/bin/env python3
"""Recompute matched timing and exclusive admission summaries from raw trials."""

import json
import statistics
from pathlib import Path

BASE = Path(__file__).resolve().parent
RUNS = BASE / "runs"


def sample(kind, variant, profile, round_number, alphabet="ascii"):
    prefix = "" if alphabet == "ascii" else f"{alphabet}-"
    name = f"{prefix}{kind}-{variant}-{profile}-{round_number:02d}"
    directory = RUNS / name
    trial = next(directory.glob("trial-*"))
    return json.loads((trial / "sample.json").read_text())


def group(kind, variant, profile, count=7, alphabet="ascii"):
    return [sample(kind, variant, profile, i, alphabet) for i in range(1, count + 1)]


def measure(values, divisor=1_000_000):
    values = [value / divisor for value in values]
    q1, _, q3 = statistics.quantiles(values, n=4, method="inclusive") if len(values) > 1 else values * 3
    return {"n": len(values), "median": statistics.median(values), "q1": q1, "q3": q3, "raw": values}


result = {"groups_ms": {}, "paired_restore_delta_ms": {}, "restore_profiles_ms": {}}
for variant in ("b0", "b1", "b2", "b3"):
    result["groups_ms"][f"restore-{variant}-off"] = measure(
        [row["load_ns"] for row in group("restore", variant, "off")]
    )
    profiles = group("restore", variant, "on", 3)
    result["restore_profiles_ms"][variant] = {
        key: measure([getter(row) for row in profiles])
        for key, getter in {
            "load": lambda row: row["load_ns"],
            "storage": lambda row: row["load_profile"]["storage"]["total_ns"],
            "inventory": lambda row: row["load_profile"]["storage"]["inventory_rebuild_ns"],
            "catalogue": lambda row: row["load_profile"]["catalogue_rebuild_ns"],
            "segment_decode": lambda row: row["load_profile"]["segment_decode_ns"],
            "bigram_validation": lambda row: sum(
                segment["decode"]["bigrams_validation_ns"]
                for segment in row["load_profile"]["segments"]
            ),
            "reconciliation_wait": lambda row: row["reconciliation_wait_ns"],
        }.items()
    }
    result["restore_profiles_ms"][variant]["domain_counts"] = [
        {
            "skipped": sum(segment["decode"].get("bigram_logical_domain_skips", 0) for segment in row["load_profile"]["segments"]),
            "checked": sum(segment["decode"].get("bigram_logical_domain_scans", 0) for segment in row["load_profile"]["segments"]),
            "fallback_checked": sum(segment["decode"].get("bigram_fallback_domain_scans", 0) for segment in row["load_profile"]["segments"]),
        }
        for row in profiles
    ]

baseline = group("restore", "b0", "off")
for variant in ("b1", "b2", "b3"):
    rows = group("restore", variant, "off")
    result["paired_restore_delta_ms"][variant] = measure(
        [current["load_ns"] - original["load_ns"] for original, current in zip(baseline, rows)]
    )

for variant in ("b0", "b3"):
    result["groups_ms"][f"construction-{variant}-off"] = measure(
        [row["build_ns"] for row in group("construction", variant, "off")]
    )
    result["groups_ms"][f"unicode-restore-{variant}-off"] = measure(
        [row["load_ns"] for row in group("restore", variant, "off", 3, "unicode")]
    )
result["groups_ms"]["construction-b3-on"] = measure(
    [row["build_ns"] for row in group("construction", "b3", "on")]
)
result["paired_construction_delta_ms"] = measure([
    current["build_ns"] - original["build_ns"]
    for original, current in zip(group("construction", "b0", "off"), group("construction", "b3", "off"))
])

construction_profiles = group("construction", "b3", "on")
fields = construction_profiles[0]["build_profile"]["admission"].keys()
result["admission"] = {
    key: measure([row["build_profile"]["admission"][key] for row in construction_profiles], 1 if not key.endswith("_ns") else 1_000_000)
    for key in fields
}
result["admission_by_collection"] = {
    collection: {
        key: measure([row["build_profile"]["admission_by_collection"][collection][key] for row in construction_profiles], 1 if not key.endswith("_ns") else 1_000_000)
        for key in fields
    }
    for collection in construction_profiles[0]["build_profile"]["admission_by_collection"]
}
exclusive = (
    "coordinator_ns", "record_inventory_ns", "postings_ns", "inference_ns",
    "relations_ns", "custom_postings_ns", "raw_bigrams_ns",
    "fallback_bigrams_ns", "custom_bigrams_ns", "structural_facts_ns", "sorting_ns",
)
result["admission_residual_percent"] = measure([
    100 * row["build_profile"]["admission"]["remainder_ns"]
    / row["build_profile"]["admission"]["total_ns"]
    for row in construction_profiles
], 1)
assert all(
    row["build_profile"]["admission"]["total_ns"]
    == sum(row["build_profile"]["admission"][field] for field in exclusive)
    + row["build_profile"]["admission"]["remainder_ns"]
    for row in construction_profiles
)
result["legacy_admission_ms"] = measure([row["build_admission_ns"] for row in construction_profiles])
result["construction_readiness_ms"] = {
    key: measure([row[key] for row in construction_profiles])
    for key in ("catalogue_observed_ns", "first_publication_observed_ns", "first_result_ns", "completion_ns")
}
result["construction_worker_ms"] = {
    key: measure([row[key] for row in construction_profiles])
    for key in ("build_job_execution_ns", "build_receive_wait_ns", "build_publication_ns")
}

result["rss_bytes"] = {}
result["io"] = {}
for kind, variants in (("construction", ("b0", "b3")), ("restore", ("b0", "b1", "b2", "b3"))):
    for variant in variants:
        rows = group(kind, variant, "off")
        name = f"{kind}-{variant}"
        keys = ("after_build_rss", "after_build_hwm") if kind == "construction" else ("after_load_rss", "after_load_hwm")
        result["rss_bytes"][name] = {key: measure([row[key] for row in rows], 1) for key in keys}
        result["io"][name] = {key: measure([row["process_io_delta"][key] for row in rows], 1) for key in ("rchar", "syscr", "read_bytes")}

all_rows = []
for trial in RUNS.glob("*/trial-*"):
    all_rows.append(json.loads((trial / "sample.json").read_text()))
result["checks"] = {
    "trial_count": len(all_rows),
    "all_restores_used_cache": all(row.get("restored_from_cache") is True for row in all_rows if "load_ns" in row),
    "distinct_query_digests": len({json.dumps(row["query_digests"], sort_keys=True) for row in all_rows if row.get("query_digests")}),
}
(BASE / "summary.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
print("wrote summary.json", result["checks"])
