#!/usr/bin/env python3
"""Reconcile B and C test inventories through the three assignment mappings."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


PREFIX_MOVES = {
    "completion::tests::": "query::completion::tests::",
    "decimal::tests::": "model::decimal::tests::",
    "detailed_timer::tests::": "measurement::timer::tests::",
    "diagnostics::tests::": "measurement::format::tests::",
    "discovery::tests::": "inventory::discovery::tests::",
    "execution_options::tests::": "query::options::tests::",
    "folder_tests::": "tests::folder::",
    "foundation_invariants::": "tests::foundation_invariants::",
    "id_tests::": "tests::id::",
    "inference::build::allocation_tests::": "construction::inference::tests::",
    "path_repr::tests::": "model::path_representation::tests::",
    "posting_values::tests::": "repository::posting_values::tests::",
    "relation_inspection::tests::": "repository::relations::tests::",
    "relation_semijoin::tests::": "query_access::relations::semijoin::tests::",
    "relation_target_cache::tests::": "query_execution::relations::target_cache::tests::",
    "streaming::tests::": "query_execution::streaming::tests::",
    "structural::tests::": "model::structure::tests::",
    "tests::root_fixture_is_available_without_archived_workspace_paths":
        "tests::fixture_layout::root_fixture_is_available_without_archived_workspace_paths",
}

ADDED_TESTS = {
    "query_execution::preindex::tests::jsonl_identity_checks_exact_ranges_classes_and_fallback_identity",
    "query_execution::preindex::tests::registered_json_uses_generated_ids_with_builtin_scalar_verification",
    "query_execution::preindex::tests::relative_path_errors_name_one_valid_file_and_exact_range",
    "query_execution::preindex::tests::relative_path_identity_checks_whole_file_ranges_and_fallback_ids",
    "source::registry::tests::registered_readers_do_not_enable_relative_path_identity",
    "tests::folder::folder_identity_defaults_and_relative_path_eligibility_follow_format",
}


def mapped_name(name: str) -> str:
    if name == "relation_query::tests::relation_work_limits_return_explicit_errors":
        return "query_execution::relations::work::tests::relation_work_limits_return_explicit_errors"
    if name.startswith("relation_query::tests::"):
        return name.replace(
            "relation_query::tests::", "query_execution::relations::evaluation::tests::", 1
        )
    if name == "record_preparation::tests::build_rejects_non_finite_numbers_before_indexing_nested_values":
        return "construction::file_preparation::tests::build_rejects_non_finite_numbers_before_indexing_nested_values"
    if name.startswith("record_preparation::tests::"):
        return name.replace("record_preparation::tests::", "source::preparation::tests::", 1)
    if name == "crates/core/src/repository/mod.rs - repository::Repository (line 40)":
        return "crates/core/src/repository/mod.rs - repository::Repository (line 42)"
    for old, new in PREFIX_MOVES.items():
        if name.startswith(old):
            return new + name[len(old) :]
    return name


def load_list(path: Path) -> list[str]:
    return [line for line in path.read_text(encoding="utf-8").splitlines() if line]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("inventory", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    profiles = {}
    for profile in ("default", "all-features"):
        baseline = load_list(args.inventory / "b" / f"{profile}.list")
        current = set(load_list(args.inventory / "c" / f"{profile}.list"))
        transformed = {mapped_name(name) for name in baseline}
        if transformed | ADDED_TESTS != current:
            raise ValueError(
                f"{profile} mismatch: "
                f"missing={sorted((transformed | ADDED_TESTS) - current)} "
                f"unmapped={sorted(current - transformed - ADDED_TESTS)}"
            )
        profiles[profile] = {
            "baseline_count": len(baseline),
            "candidate_count": len(current),
            "moved_count": sum(mapped_name(name) != name for name in baseline),
            "added_test_count": len(ADDED_TESTS),
            "unmapped_baseline_count": 0,
            "ignored_equal": load_list(
                args.inventory / "b" / f"{profile}-ignored.list"
            )
            == load_list(args.inventory / "c" / f"{profile}-ignored.list"),
            "ignored_count": len(
                load_list(args.inventory / "c" / f"{profile}-ignored.list")
            ),
        }
        if not profiles[profile]["ignored_equal"]:
            raise ValueError(f"{profile} ignored sets differ")

    result = {
        "prefix_moves": PREFIX_MOVES,
        "single_leaf_moves": {
            "relation_query::tests::relation_work_limits_return_explicit_errors":
                "query_execution::relations::work::tests::relation_work_limits_return_explicit_errors",
            "record_preparation::tests::build_rejects_non_finite_numbers_before_indexing_nested_values":
                "construction::file_preparation::tests::build_rejects_non_finite_numbers_before_indexing_nested_values",
        },
        "record_preparation_split": {
            "construction::file_preparation::tests": 1,
            "source::preparation::tests": 5,
        },
        "relation_query_split": {
            "query_execution::relations::evaluation::tests": 32,
            "query_execution::relations::work::tests": 1,
        },
        "added_tests": sorted(ADDED_TESTS),
        "relocated_doctest": {
            "baseline": "crates/core/src/repository/mod.rs - repository::Repository (line 40)",
            "candidate": "crates/core/src/repository/mod.rs - repository::Repository (line 42)",
        },
        "profiles": profiles,
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
