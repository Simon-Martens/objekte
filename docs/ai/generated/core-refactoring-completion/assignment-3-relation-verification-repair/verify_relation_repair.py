#!/usr/bin/env python3
"""Verify the deterministic relation traversal and archived posting probes."""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path


A2_REVISION = "9ae617b5df3f0a082aa923195a28019d5c36815d"
ARCHIVED_C_REVISION = "acac94f486ee0f9d35eb5ba834d26cdc01eb1c5e"
FINAL_REVISION = "fd25d8678af5161cb92647d7dbddefa7f1aff391"
TRAVERSAL_CASES = (
    "indexed_relation_filter",
    "selected_relation_fts",
    "source_dependent_residual_sort",
)
ARCHIVED_CASES = {
    "selected_and_residual": ("Q102", "Q122"),
    "typed_id_field": ("Q088",),
}
ARCHIVED_DIRS = {
    "selected_and_residual": "relation-a2-to-c-extended",
    "typed_id_field": "relation-a2-to-c-extended-hops",
}
REPETITIONS = (0, 1, 2)


def load_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def load_jsonl(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]


def parse_probe_log(path: Path) -> list[dict]:
    result = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("RELATION_PROBE "):
            result.append(json.loads(line.removeprefix("RELATION_PROBE ")))
    return result


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def validate_manifest(path: Path, side: str, expected_revision: str) -> dict:
    manifest = load_json(path)
    require(manifest.get("side") == side, f"{side} run manifest has wrong side")
    require(
        manifest.get("revision") == expected_revision,
        f"{side} run manifest has wrong revision",
    )
    require(
        tuple(manifest.get("expected_cases", ())) == TRAVERSAL_CASES,
        f"{side} run manifest has wrong cases",
    )
    require(
        tuple(manifest.get("expected_repetitions", ())) == (0,),
        f"{side} run manifest has wrong repetitions",
    )
    require(manifest.get("alphabet") == "Ascii", f"{side} run manifest has wrong alphabet")
    require(manifest.get("query_workers") == 32, f"{side} run manifest has wrong workers")
    require(
        manifest.get("verified_cache_bytes") == 0,
        f"{side} run manifest has wrong verified cache",
    )
    require(
        manifest.get("publication_state") == "fresh deterministic fixture",
        f"{side} run manifest has wrong publication state",
    )
    return manifest


def selected_kind(observation: dict) -> str:
    terms = observation["plan"].get("terms", [])
    require(bool(terms), f"{observation['case_id']} has no selected term")
    return terms[0]["selected"]["kind"]


def ids(observation: dict) -> list[str]:
    return [hit["record_id"] for hit in observation["result"]["hits"]]


def validate_success(status: dict, case_id: str, side: str) -> None:
    require(
        status.get("status") == "ok",
        f"{side} {case_id} has failed status: {status.get('error')!r}",
    )
    require(status.get("error") is None, f"{side} {case_id} has an error")


def validate_traversal_case(observation: dict, side: str) -> None:
    case_id = observation["case_id"]
    validate_success(observation["result"], case_id, side)
    plan = observation["plan"]
    counters = observation["result"]["counters"]

    if case_id == "indexed_relation_filter":
        validate_success(observation["result"], case_id, side)
        require(plan["fts_strategy"].startswith("relation target-first"), "wrong filter route")
        require(selected_kind(observation) == "relation_target_first", "wrong filter strategy")
        require(ids(observation) == [f"S{index:02}" for index in range(40)], "wrong filter IDs")
        require(
            all(hit["matched_fields"] == ["entry.title"] for hit in observation["result"]["hits"]),
            "wrong filter evidence",
        )
        memberships = {
            (item["candidate_records"], item["proven_records"])
            for item in plan["observations"]
        }
        require((40, 40) in memberships, "filter proof membership missing")
        expected = {
            "relation_target_reads": 0,
            "relation_target_file_opens": 0,
            "relation_edge_visits": 1,
            "relation_bytes_read": 0,
            "relation_target_candidates": 1,
            "relation_target_matches": 1,
            "relation_reverse_postings_visited": 1,
            "relation_reverse_members_visited": 40,
            "scanned_records": 0,
        }
        for field, value in expected.items():
            require(counters[field] == value, f"filter {field} is {counters[field]}, expected {value}")
    elif case_id == "selected_relation_fts":
        require(
            plan["fts_strategy"].startswith("relation source-restricted"),
            "wrong selected FTS route",
        )
        require(selected_kind(observation) == "relation_source_restricted", "wrong FTS strategy")
        require(ids(observation) == ["S00"], "wrong selected FTS IDs")
        require(
            observation["result"]["hits"][0]["matched_fields"]
            == ["entry.title", "title"],
            "wrong selected FTS evidence",
        )
        expected = {
            "relation_target_reads": 1,
            "relation_target_file_opens": 1,
            "relation_edge_visits": 1,
            "relation_bytes_read": 56,
            "relation_target_candidates": 1,
            "relation_target_matches": 1,
            "relation_reverse_postings_visited": 1,
            "relation_reverse_members_visited": 40,
            "scanned_records": 0,
        }
        for field, value in expected.items():
            require(counters[field] == value, f"selected FTS {field} is {counters[field]}, expected {value}")
    elif case_id == "source_dependent_residual_sort":
        require(plan["sort_strategy"] == "bounded_top_k", "wrong residual sort strategy")
        require(selected_kind(observation) == "relation_target_first", "wrong residual route")
        require(ids(observation) == ["m8", "m1"], "wrong residual IDs")
        require(
            observation["result"]["hits"] == observation["reference"]["hits"],
            "residual result differs from source-backed reference",
        )
        validate_success(observation["reference"], "reference residual", side)
        require(
            all(
                hit["matched_fields"] == ["entry.title"]
                for hit in observation["result"]["hits"]
            ),
            "wrong residual evidence",
        )
        expected = {
            "relation_target_reads": 2,
            "relation_target_file_opens": 2,
            "relation_edge_visits": 6,
            "relation_bytes_read": 93,
            "relation_verified_targets_reused": 1,
            "residual_evaluations": 9,
            "sort_target_reads": 2,
            "files_opened": 13,
            "record_parses": 12,
            "bytes_read": 628,
            "scanned_records": 9,
        }
        for field, value in expected.items():
            require(counters[field] == value, f"residual {field} is {counters[field]}, expected {value}")
    else:
        raise ValueError(f"unexpected traversal case {case_id}")


def validate_traversal_side(observations: list[dict], revision: str, side: str) -> dict:
    keys = Counter((row.get("case_id"), row.get("repetition")) for row in observations)
    expected = {(case_id, 0) for case_id in TRAVERSAL_CASES}
    missing = sorted(expected - set(keys))
    require(not missing, f"{side} missing expected case/repetition: {missing}")
    unexpected = sorted(set(keys) - expected)
    require(not unexpected, f"{side} has unexpected case/repetition: {unexpected}")
    duplicates = sorted(key for key, count in keys.items() if count != 1)
    require(not duplicates, f"{side} has duplicate observations/plans: {duplicates}")

    indexed = {}
    for observation in observations:
        require(observation.get("revision") == revision, f"{side} has wrong observation revision")
        require(observation.get("alphabet") == "Ascii", f"{side} has wrong alphabet")
        require(observation.get("query_workers") == 32, f"{side} has wrong query workers")
        require(isinstance(observation.get("plan"), dict), f"{side} has a missing plan")
        validate_traversal_case(observation, side)
        indexed[observation["case_id"]] = observation
    return indexed


def archived_sample_key(row: dict) -> tuple[str, int]:
    profile = row["profile"]
    require(profile["alphabet"] == "ascii", "archived sample is not ASCII")
    require(profile["warm"] is False, "archived sample is warm")
    require(profile["verified_cache_bytes"] == 0, "archived sample has verified cache")
    return row["case_id"], row["repetition"]


def load_archived_samples(path: Path) -> dict[tuple[str, int], dict]:
    samples = {}
    for row in load_jsonl(path):
        key = archived_sample_key(row)
        require(key not in samples, f"archived sample duplicate: {key}")
        samples[key] = row
    return samples


def load_archived_plans(path: Path) -> dict[str, dict]:
    plans = {}
    for row in load_jsonl(path):
        case_id = row["case_id"]
        require(case_id not in plans, f"archived {case_id} expected one plan")
        plans[case_id] = row
    return plans


def validate_archived_revision(manifest: dict, side: str, field: str, expected: str) -> None:
    observed = manifest.get(field)
    require(
        observed == expected,
        f"archived {side} run manifest {field} expected {expected}, observed {observed!r}",
    )


def load_archived_digests(
    path: Path, side: str, expected_cases: tuple[str, ...]
) -> dict[str, str]:
    def unique_object(pairs: list[tuple[str, object]]) -> dict:
        result = {}
        for key, value in pairs:
            require(
                key not in result,
                f"archived {side} {path.name} duplicate key {key}",
            )
            result[key] = value
        return result

    try:
        digests = json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=unique_object,
        )
    except json.JSONDecodeError as error:
        raise ValueError(
            f"archived {side} {path.name} invalid JSON: {error}"
        ) from error
    require(
        isinstance(digests, dict),
        f"archived {side} {path.name} must contain an object",
    )
    expected = set(expected_cases)
    missing = sorted(expected - set(digests))
    require(
        not missing,
        f"archived {side} {path.name} missing case IDs: {missing}",
    )
    unexpected = sorted(set(digests) - expected)
    require(
        not unexpected,
        f"archived {side} {path.name} unexpected case IDs: {unexpected}",
    )
    for case_id in expected_cases:
        digest = digests[case_id]
        require(
            isinstance(digest, str) and re.fullmatch(r"[0-9a-f]{64}", digest) is not None,
            f"archived {side} {path.name} {case_id} malformed digest: {digest!r}",
        )
    return digests


def validate_archived_probe(root: Path, expected_cases: tuple[str, ...]) -> dict:
    old_dir = root / "a2/queries"
    new_dir = root / "c/queries"
    old_manifest = load_json(old_dir / "run-manifest.json")
    new_manifest = load_json(new_dir / "run-manifest.json")
    validate_archived_revision(old_manifest, "A2", "code_revision", A2_REVISION)
    validate_archived_revision(
        old_manifest, "A2", "binary_build_revision", A2_REVISION
    )
    validate_archived_revision(
        new_manifest, "C", "code_revision", ARCHIVED_C_REVISION
    )
    validate_archived_revision(
        new_manifest,
        "C",
        "binary_build_revision",
        ARCHIVED_C_REVISION,
    )
    require(old_manifest["query_workers"] == new_manifest["query_workers"] == 4, "archived workers differ")

    old = load_archived_samples(old_dir / "measurements.jsonl")
    new = load_archived_samples(new_dir / "measurements.jsonl")
    expected_keys = {(case_id, repetition) for case_id in expected_cases for repetition in REPETITIONS}
    for side, samples in (("A2", old), ("C", new)):
        missing = sorted(expected_keys - set(samples))
        require(not missing, f"archived {side} missing expected case/repetition: {missing}")
        unexpected = sorted(set(samples) - expected_keys)
        require(not unexpected, f"archived {side} has unexpected case/repetition: {unexpected}")
        for key, sample in samples.items():
            validate_success(sample, f"{side} archived {key}", side)

    old_plans = load_archived_plans(old_dir / "query-plans.jsonl")
    new_plans = load_archived_plans(new_dir / "query-plans.jsonl")
    require(set(old_plans) == set(new_plans) == set(expected_cases), "archived plan cases differ")
    old_digests = load_archived_digests(
        old_dir / "result-digests.json", "A2", expected_cases
    )
    new_digests = load_archived_digests(
        new_dir / "result-digests.json", "C", expected_cases
    )
    require(old_digests == new_digests, "archived ordered results differ")

    for case_id in expected_cases:
        for plans, side in ((old_plans, "A2"), (new_plans, "C")):
            traces = plans[case_id]["plans"]
            require(len(traces) == 1, f"archived {side} {case_id} expected one plan")
            selected = traces[0]["terms"][0]["selected"]["kind"]
            require(selected == "posting", f"archived {side} {case_id} is not a posting")
        for key in ((case_id, repetition) for repetition in REPETITIONS):
            require(old[key]["counters"] == new[key]["counters"], f"archived counter mismatch: {key}")
            counters = new[key]["counters"]
            require(counters["relation_target_reads"] == 0, f"archived {case_id} has target reads")
            require(counters["relation_edge_visits"] == 0, f"archived {case_id} has edge visits")

    schema_bytes_equal = (
        old_dir / "schema-manifest.json"
    ).read_bytes() == (new_dir / "schema-manifest.json").read_bytes()
    return {
        "cases": list(expected_cases),
        "samples": len(old),
        "matched_samples": len(set(old) & set(new)),
        "ordered_result_matches": len(old_digests),
        "counter_mismatches": 0,
        "posting_classification_verified": True,
        "schema_manifests_equal": schema_bytes_equal,
        "schema_compatibility_check": "not_accepted_extended_adapter_in_either_run",
    }


def validate_evidence(evidence: Path, archived: Path) -> dict:
    old_manifest = validate_manifest(evidence / "a2-run-manifest.json", "a2", A2_REVISION)
    new_manifest = validate_manifest(evidence / "final-run-manifest.json", "final", FINAL_REVISION)
    old = validate_traversal_side(
        parse_probe_log(evidence / old_manifest["log"]), A2_REVISION, "A2"
    )
    new = validate_traversal_side(
        parse_probe_log(evidence / new_manifest["log"]), FINAL_REVISION, "final"
    )
    require(old.keys() == new.keys(), "traversal case sets differ")
    for case_id in TRAVERSAL_CASES:
        require(old[case_id]["result"] == new[case_id]["result"], f"{case_id} result/work differs")
        require(old[case_id]["plan"] == new[case_id]["plan"], f"{case_id} plan differs")
        if "reference" in old[case_id]:
            require(
                old[case_id]["reference"] == new[case_id]["reference"],
                f"{case_id} reference differs",
            )

    archived_results = {
        name: validate_archived_probe(archived / ARCHIVED_DIRS[name], cases)
        for name, cases in ARCHIVED_CASES.items()
    }
    return {
        "traversal": {
            "cases": list(TRAVERSAL_CASES),
            "samples_per_side": len(old),
            "sample_records": len(old) + len(new),
            "paired_samples": len(old),
            "work_fields_compared": sorted(old["indexed_relation_filter"]["result"]["counters"]),
            "result_mismatches": 0,
            "plan_mismatches": 0,
            "cases": {
                case_id: {
                    "route": selected_kind(new[case_id]),
                    "fts_strategy": new[case_id]["plan"]["fts_strategy"],
                    "sort_strategy": new[case_id]["plan"]["sort_strategy"],
                    "ordered_ids": ids(new[case_id]),
                    "matched_fields": [
                        hit["matched_fields"] for hit in new[case_id]["result"]["hits"]
                    ],
                    "counters": new[case_id]["result"]["counters"],
                    "candidate_proven_membership": [
                        {
                            "candidate_records": item["candidate_records"],
                            "proven_records": item["proven_records"],
                        }
                        for item in new[case_id]["plan"]["observations"]
                    ],
                }
                for case_id in TRAVERSAL_CASES
            },
        },
        "archived_posting_probes": archived_results,
    }


def rewrite_probe_log(path: Path, rows: list[dict]) -> None:
    body = "".join(
        "RELATION_PROBE " + json.dumps(row, separators=(",", ":")) + "\n"
        for row in rows
    )
    path.write_text(body, encoding="utf-8")


def run_child(evidence: Path, archived: Path) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, __file__, str(evidence), "--archived", str(archived)],
        text=True,
        capture_output=True,
        check=False,
    )


def make_case(evidence: Path, archived: Path, root: Path) -> tuple[Path, Path]:
    case_evidence = root / "evidence"
    case_archived = root / "archived"
    shutil.copytree(evidence, case_evidence)
    shutil.copytree(archived, case_archived)
    return case_evidence, case_archived


def require_invalid(
    evidence: Path,
    archived: Path,
    root: Path,
    name: str,
    phrase: str,
    mutate,
) -> None:
    case_root = root / name
    case_evidence, case_archived = make_case(evidence, archived, case_root)
    mutate(case_evidence, case_archived)
    child = run_child(case_evidence, case_archived)
    require(child.returncode != 0, f"self-test {name} unexpectedly succeeded")
    require(
        phrase in child.stderr,
        f"self-test {name} failed for the wrong reason: {child.stderr}",
    )
    print(f"negative control {name}: rejected ({phrase})")


def self_test(evidence: Path, archived: Path) -> None:
    valid = run_child(evidence, archived)
    require(valid.returncode == 0, f"valid control failed: {valid.stderr}")
    print("valid control: accepted")

    with tempfile.TemporaryDirectory(prefix="objekte-relation-repair-verifier-") as temp:
        root = Path(temp)

        def remove_case(case_evidence: Path, _case_archived: Path) -> None:
            for name in ("a2-relation-probes.log", "final-relation-probes.log"):
                rows = [
                    row
                    for row in parse_probe_log(case_evidence / name)
                    if row["case_id"] != "selected_relation_fts"
                ]
                rewrite_probe_log(case_evidence / name, rows)

        require_invalid(
            evidence,
            archived,
            root,
            "remove-case",
            "missing expected case/repetition",
            remove_case,
        )

        def remove_repetition(case_evidence: Path, _case_archived: Path) -> None:
            for name in ("a2-relation-probes.log", "final-relation-probes.log"):
                rows = parse_probe_log(case_evidence / name)
                for row in rows:
                    row.pop("repetition", None)
                rewrite_probe_log(case_evidence / name, rows)

        require_invalid(
            evidence,
            archived,
            root,
            "remove-repetition",
            "missing expected case/repetition",
            remove_repetition,
        )

        def duplicate_plan(_case_evidence: Path, case_archived: Path) -> None:
            path = (
                case_archived
                / ARCHIVED_DIRS["typed_id_field"]
                / "c/queries/query-plans.jsonl"
            )
            lines = path.read_text(encoding="utf-8").splitlines()
            path.write_text("\n".join([*lines, lines[0]]) + "\n", encoding="utf-8")

        require_invalid(
            evidence,
            archived,
            root,
            "duplicate-plan",
            "expected one plan",
            duplicate_plan,
        )

        def empty_evidence(case_evidence: Path, case_archived: Path) -> None:
            (case_evidence / "a2-relation-probes.log").write_text("", encoding="utf-8")
            (case_evidence / "final-relation-probes.log").write_text("", encoding="utf-8")
            for scenario in ARCHIVED_CASES:
                for side in ("a2", "c"):
                    result = case_archived / ARCHIVED_DIRS[scenario] / side / "queries"
                    (result / "measurements.jsonl").write_text("", encoding="utf-8")
                    (result / "query-plans.jsonl").write_text("", encoding="utf-8")
                    (result / "result-digests.json").write_text("{}\n", encoding="utf-8")

        require_invalid(
            evidence,
            archived,
            root,
            "empty-both-sides",
            "missing expected case/repetition",
            empty_evidence,
        )

        def fail_both_sides(case_evidence: Path, _case_archived: Path) -> None:
            for name in ("a2-relation-probes.log", "final-relation-probes.log"):
                rows = parse_probe_log(case_evidence / name)
                for row in rows:
                    if row["case_id"] == "indexed_relation_filter":
                        row["result"]["status"] = "failed"
                        row["result"]["error"] = "injected query failure"
                rewrite_probe_log(case_evidence / name, rows)

        require_invalid(
            evidence,
            archived,
            root,
            "same-failed-query",
            "failed status",
            fail_both_sides,
        )

        def duplicate_archived_digest(_case_evidence: Path, case_archived: Path) -> None:
            path = (
                case_archived
                / ARCHIVED_DIRS["typed_id_field"]
                / "a2/queries/result-digests.json"
            )
            digests = load_json(path)
            committed = digests["Q088"]
            conflicting = "0" * 64 if committed != "0" * 64 else "1" * 64
            text = path.read_text(encoding="utf-8")
            needle = f'  "Q088": {json.dumps(committed)}'
            require(needle in text, "self-test could not find Q088 digest")
            replacement = (
                f'  "Q088": {json.dumps(conflicting)},\n'
                f'  "Q088": {json.dumps(committed)}'
            )
            path.write_text(text.replace(needle, replacement, 1), encoding="utf-8")

        require_invalid(
            evidence,
            archived,
            root,
            "duplicate-archived-digest",
            "A2 result-digests.json duplicate key Q088",
            duplicate_archived_digest,
        )

        def null_archived_digest(_case_evidence: Path, case_archived: Path) -> None:
            for side in ("a2", "c"):
                path = (
                    case_archived
                    / ARCHIVED_DIRS["typed_id_field"]
                    / side
                    / "queries/result-digests.json"
                )
                digests = load_json(path)
                digests["Q088"] = None
                path.write_text(json.dumps(digests, indent=2) + "\n", encoding="utf-8")

        require_invalid(
            evidence,
            archived,
            root,
            "null-archived-digest",
            "Q088 malformed digest",
            null_archived_digest,
        )

        def wrong_binary_revision(_case_evidence: Path, case_archived: Path) -> None:
            path = (
                case_archived
                / ARCHIVED_DIRS["typed_id_field"]
                / "c/queries/run-manifest.json"
            )
            manifest = load_json(path)
            manifest["binary_build_revision"] = "0" * 40
            path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

        require_invalid(
            evidence,
            archived,
            root,
            "wrong-binary-revision",
            "C run manifest binary_build_revision",
            wrong_binary_revision,
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("evidence", type=Path)
    parser.add_argument("--archived", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    try:
        result = validate_evidence(args.evidence.resolve(), args.archived.resolve())
        if args.output:
            args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        if args.self_test:
            self_test(args.evidence.resolve(), args.archived.resolve())
        print(json.dumps(result, indent=2))
        return 0
    except (KeyError, TypeError, ValueError, OSError, json.JSONDecodeError) as error:
        print(f"relation verification failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
