#!/usr/bin/env python3
"""Summarize the final, sequential profiling campaign from raw JSON samples."""

import json
import statistics
from pathlib import Path

BASE = Path(__file__).resolve().parent


def samples(prefix):
    paths = sorted(BASE.glob(f"{prefix}-*/trial-*/sample.json"))
    return [(path, json.loads(path.read_text())) for path in paths]


def measure(values, nanos=True):
    values = [value / 1_000_000 if nanos else value for value in values]
    quartiles = statistics.quantiles(values, n=4, method="inclusive") if len(values) > 1 else values * 3
    return {
        "n": len(values),
        "median": statistics.median(values),
        "q1": quartiles[0],
        "q3": quartiles[2],
        "raw": values,
    }


def nested(row, key):
    for part in key.split("."):
        row = row[part]
    return row


groups = {}
for name, key in (
    ("construction-off", "build_ns"),
    ("construction-on", "build_ns"),
    ("restore-off", "load_ns"),
    ("restore-on", "load_ns"),
    ("workers-1", "build_ns"),
    ("workers-8", "build_ns"),
    ("workers-32", "build_ns"),
    ("unicode", "build_ns"),
    ("plain", "build_ns"),
    ("inference", "build_ns"),
):
    rows = samples(name)
    groups[name] = measure([row[key] for _, row in rows])

construction = [row for _, row in samples("construction-on")]
restore = [row for _, row in samples("restore-on")]
assert len(construction) == len(restore) == 7
construction_outer = [
    "build_profile.pool_init_ns", "build_discovery_ns", "build_profile.builder_setup_ns",
    "build_profile.job_scheduling_ns", "build_extraction_index_ns",
    "build_profile.final_segment_flush_ns", "build_finalization_ns",
]
restore_outer = [
    "load_profile.storage.total_ns", "load_profile.extra_metadata_ns",
    "load_profile.catalogue_rebuild_ns", "load_profile.file_versions_ns",
    "load_profile.base_active_validation_ns", "load_profile.segment_decode_ns",
    "load_profile.cross_segment_uniqueness_ns", "load_profile.publication_setup_ns",
    "load_profile.reconciliation_schedule_ns",
]
storage_outer = [
    "load_profile.storage.header_configuration_ns",
    "load_profile.storage.inventory_rebuild_ns",
    "load_profile.storage.collection_validation_ns",
    "load_profile.storage.relation_validation_ns",
    "load_profile.storage.lifecycle_validation_ns",
    "load_profile.storage.repository_setup_ns",
]


def measurements(rows, fields):
    return {key: measure([nested(row, key) for row in rows]) for key in fields}


def residual(rows, total, fields):
    return measure([nested(row, total) - sum(nested(row, field) for field in fields) for row in rows])


result = {
    "groups_ms": groups,
    "construction_outer_ms": measurements(construction, construction_outer),
    "construction_residual_ms": residual(construction, "build_ns", construction_outer),
    "restore_outer_ms": measurements(restore, restore_outer),
    "restore_residual_ms": residual(restore, "load_ns", restore_outer),
    "storage_outer_ms": measurements(restore, storage_outer),
    "storage_sections_ms": measure([sum(section["total_ns"] for section in row["load_profile"]["storage"]["sections"]) for row in restore]),
    "construction_detail_ms": measurements(construction, [
        "build_publication_ns", "build_job_execution_ns", "build_receive_wait_ns",
        "build_admission_ns", "build_profile.publication_index_ns",
        "build_profile.publication_install_ns", "build_profile.publication_callback_ns",
        "build_profile.discovery_enumeration_ns", "build_profile.discovery_metadata_prepare_ns",
        "build_profile.discovery_ordered_insert_ns", "build_profile.fingerprint_finish_ns",
        "catalogue_observed_ns", "first_publication_observed_ns", "first_result_ns",
        "first_result_probe_ns", "completion_ns", "caller_return_ns",
    ]),
    "restore_detail_ms": measurements(restore, [
        "load_profile.storage.total_ns", "reconciliation_wait_ns", "reconciliation_done_ns",
    ]),
}

for name, rows in (("construction", construction), ("restore", restore)):
    key = "build_profile" if name == "construction" else "load_profile"
    result[f"{name}_rss_bytes"] = measurements(rows, ["after_build_rss", "after_build_hwm"] if name == "construction" else ["after_load_rss", "after_load_hwm"])
    result[f"{name}_process_io"] = {field: measure([row["process_io_delta"][field] for row in rows], False) for field in ("rchar", "syscr", "read_bytes")}

result["query_digests_agree"] = len({json.dumps(row["query_digests"], sort_keys=True) for prefix in ("construction-off", "construction-on", "restore-off", "restore-on") for _, row in samples(prefix)}) == 1
result["restore_all_from_cache"] = all(row["restored_from_cache"] for prefix in ("restore-off", "restore-on") for _, row in samples(prefix))
result["construction_profile"] = construction[0]["build_profile"]
result["restore_profile"] = restore[0]["load_profile"]
result["construction_manifest"] = json.loads(next(BASE.glob("construction-on-1/trial-*/run-manifest.json")).read_text())
result["restore_manifest"] = json.loads(next(BASE.glob("restore-on-1/trial-*/run-manifest.json")).read_text())
(BASE / "summary.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
print("wrote", BASE / "summary.json")
