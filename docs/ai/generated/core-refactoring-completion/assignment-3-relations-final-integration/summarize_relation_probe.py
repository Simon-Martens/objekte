#!/usr/bin/env python3
"""Compare the extended-schema A2 and C relation probes."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from statistics import median


def load_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def load_jsonl(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]


def samples(result_dir: Path) -> dict[tuple[str, int], dict]:
    rows = {}
    for row in load_jsonl(result_dir / "measurements.jsonl"):
        key = (row["case_id"], row["repetition"])
        if key in rows:
            raise ValueError(f"duplicate sample {key} in {result_dir}")
        rows[key] = row
    return rows


def plans(result_dir: Path) -> dict[str, dict]:
    return {row["case_id"]: row for row in load_jsonl(result_dir / "query-plans.jsonl")}


def normalized_plan(row: dict) -> dict:
    copy = json.loads(json.dumps(row))
    copy.pop("diagnostic_ns", None)
    copy.pop("diagnostic_counters", None)
    return copy


def summarize_probe(root: Path) -> dict:
    old = samples(root / "a2/queries")
    new = samples(root / "c/queries")
    if old.keys() != new.keys():
        raise ValueError(f"sample keys differ under {root}")
    if not all(
        old[key]["status"] == new[key]["status"]
        and old[key]["error"] == new[key]["error"]
        and old[key]["counters"] == new[key]["counters"]
        for key in old
    ):
        raise ValueError(f"sample results differ under {root}")

    old_digests = load_json(root / "a2/queries/result-digests.json")
    new_digests = load_json(root / "c/queries/result-digests.json")
    if old_digests != new_digests:
        raise ValueError(f"ordered result digests differ under {root}")
    if (
        root / "a2/queries/schema-manifest.json"
    ).read_bytes() != (root / "c/queries/schema-manifest.json").read_bytes():
        raise ValueError(f"schema manifests differ under {root}")

    old_plans = plans(root / "a2/queries")
    new_plans = plans(root / "c/queries")
    if {
        case_id: normalized_plan(row) for case_id, row in old_plans.items()
    } != {case_id: normalized_plan(row) for case_id, row in new_plans.items()}:
        raise ValueError(f"query plans differ under {root}")

    cases = {}
    for case_id in sorted(old_digests):
        old_times = [
            row["total_ns"] for (sample_case, _), row in old.items() if sample_case == case_id
        ]
        new_times = [
            row["total_ns"] for (sample_case, _), row in new.items() if sample_case == case_id
        ]
        plan = new_plans[case_id]["plans"][0]
        counters = new[(case_id, 0)]["counters"]
        term = plan["terms"][0]
        cases[case_id] = {
            "ordered_result_digest": new_digests[case_id],
            "a2_median_ns": round(median(old_times)),
            "c_median_ns": round(median(new_times)),
            "input_records": plan["input_records"],
            "candidates": plan["candidates"],
            "candidate_records": [item["candidate_records"] for item in plan["observations"]],
            "proven_records": [item["proven_records"] for item in plan["observations"]],
            "filter_exact": plan["filter_exact"],
            "remaining_verification": plan["remaining_verification"],
            "selected_access": term["selected"],
            "alternatives": term["alternatives"],
            "counters": {
                "total_matches": counters["total_matches"],
                "returned_records": counters["returned_records"],
                "relation_source_reads": counters["relation_source_reads"],
                "relation_target_reads": counters["relation_target_reads"],
                "relation_bytes_read": counters["relation_bytes_read"],
                "relation_edge_visits": counters["relation_edge_visits"],
                "relation_target_candidates": counters["relation_target_candidates"],
                "relation_target_matches": counters["relation_target_matches"],
                "relation_reverse_postings_visited": counters[
                    "relation_reverse_postings_visited"
                ],
                "relation_reverse_members_visited": counters[
                    "relation_reverse_members_visited"
                ],
                "relation_verified_targets_reused": counters[
                    "relation_verified_targets_reused"
                ],
            },
        }
    return {
        "samples": len(old),
        "matched_samples": len(set(old) & set(new)),
        "ordered_result_cases": len(old_digests),
        "ordered_result_matches": sum(
            old_digests[case_id] == new_digests.get(case_id) for case_id in old_digests
        ),
        "counter_mismatches": 0,
        "status_mismatches": 0,
        "plan_mismatches": 0,
        "schema_manifests_equal": True,
        "cases": cases,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("evidence", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    result = {
        "selected_and_residual": summarize_probe(
            args.evidence / "relation-a2-to-c-extended"
        ),
        "typed_id_field": summarize_probe(
            args.evidence / "relation-a2-to-c-extended-hops"
        ),
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
