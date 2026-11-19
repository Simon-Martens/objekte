#!/usr/bin/env python3

import argparse
import copy
import json
import subprocess
import sys
import tempfile
from pathlib import Path


A1_SHA = "b4172a1749750ee1c87ef74a56f5ee7cec6c40ee"
CANDIDATE_SHA = "d07b700a7188efd6b45ec542f197b78a211cb88a"
COUNTER_FIELDS = (
    "files_opened",
    "distinct_files",
    "bytes_read",
    "record_parses",
    "candidates",
    "scanned_records",
    "residual_evaluations",
    "selected_payload_reads",
    "selected_payload_bytes",
)
EXPECTED = {
    "exact_page": {
        "count": 2,
        "ids": ["path:00.json"],
        "files_opened": 1,
        "distinct_files": 1,
        "bytes_read": 0,
        "record_parses": 1,
        "candidates": 2,
        "scanned_records": 0,
        "residual_evaluations": 0,
        "selected_payload_reads": 0,
        "selected_payload_bytes": 0,
    },
    "combined_old": {
        "count": 3,
        "ids": ["path:00.json", "path:01.json", "path:03.json"],
        "files_opened": 3,
        "distinct_files": 6,
        "bytes_read": 165,
        "record_parses": 3,
        "candidates": 3,
        "scanned_records": 3,
        "residual_evaluations": 3,
        "selected_payload_reads": 3,
        "selected_payload_bytes": 165,
    },
    "combined_number": {
        "count": 2,
        "ids": ["path:00.json", "path:01.json"],
        "files_opened": 3,
        "distinct_files": 5,
        "bytes_read": 167,
        "record_parses": 3,
        "candidates": 3,
        "scanned_records": 3,
        "residual_evaluations": 3,
        "selected_payload_reads": 2,
        "selected_payload_bytes": 119,
    },
}
EXPECTED_TRACE = {
    "exact_page": {
        "opens": ["data/00.json"],
        "reads": [{"path": "data/00.json", "offset": 0, "bytes": 60}],
        "open_count": 1,
        "distinct_paths": ["data/00.json"],
        "read_count": 1,
        "bytes_read": 60,
    },
    "combined_old": {
        "opens": [
            "data/00.json",
            "data/01.json",
            "data/03.json",
            "data/00.json",
            "data/01.json",
            "data/03.json",
        ],
        "reads": [
            {"path": "data/00.json", "offset": 0, "bytes": 60},
            {"path": "data/01.json", "offset": 0, "bytes": 59},
            {"path": "data/03.json", "offset": 0, "bytes": 46},
            {"path": "data/00.json", "offset": 0, "bytes": 60},
            {"path": "data/01.json", "offset": 0, "bytes": 59},
            {"path": "data/03.json", "offset": 0, "bytes": 46},
        ],
        "open_count": 6,
        "distinct_paths": ["data/00.json", "data/01.json", "data/03.json"],
        "read_count": 6,
        "bytes_read": 330,
    },
    "combined_number": {
        "opens": [
            "data/00.json",
            "data/01.json",
            "data/02.json",
            "data/00.json",
            "data/01.json",
        ],
        "reads": [
            {"path": "data/00.json", "offset": 0, "bytes": 60},
            {"path": "data/01.json", "offset": 0, "bytes": 59},
            {"path": "data/02.json", "offset": 0, "bytes": 48},
            {"path": "data/00.json", "offset": 0, "bytes": 60},
            {"path": "data/01.json", "offset": 0, "bytes": 59},
        ],
        "open_count": 5,
        "distinct_paths": ["data/00.json", "data/01.json", "data/02.json"],
        "read_count": 5,
        "bytes_read": 286,
    },
}


class VerificationError(Exception):
    pass


def require(condition, revision, query, field, message):
    if not condition:
        location = revision
        if query is not None:
            location += f"/{query}"
        if field is not None:
            location += f"/{field}"
        raise VerificationError(f"{location}: {message}")


def validate_revision(revision, document, expected_sha):
    require(isinstance(document, dict), revision, None, "root", "must be an object")
    require(
        document.get("tested_sha") == expected_sha,
        revision,
        None,
        "tested_sha",
        f"expected {expected_sha}, found {document.get('tested_sha')!r}",
    )
    results = document.get("results")
    require(isinstance(results, list), revision, None, "results", "must be a list")
    require(
        len(results) == len(EXPECTED),
        revision,
        None,
        "results",
        f"expected {len(EXPECTED)} entries, found {len(results)}",
    )

    by_name = {}
    for index, result in enumerate(results):
        require(
            isinstance(result, dict),
            revision,
            None,
            f"results[{index}]",
            "must be an object",
        )
        name = result.get("name")
        require(
            name in EXPECTED,
            revision,
            name if isinstance(name, str) else f"results[{index}]",
            "name",
            f"unknown or missing query name {name!r}",
        )
        require(
            name not in by_name,
            revision,
            name,
            "name",
            "duplicate query result",
        )
        by_name[name] = result
        counters = result.get("counters")
        require(
            isinstance(counters, dict),
            revision,
            name,
            "counters",
            "must be an object",
        )
        required_fields = {"name", "count", "ids", "counters"}
        require(
            set(result) == required_fields,
            revision,
            name,
            "fields",
            f"expected {sorted(required_fields)}, found {sorted(result)}",
        )
        require(
            set(counters) == set(COUNTER_FIELDS),
            revision,
            name,
            "counters.fields",
            f"expected {list(COUNTER_FIELDS)}, found {sorted(counters)}",
        )

        expected = EXPECTED[name]
        require(
            result["count"] == expected["count"],
            revision,
            name,
            "count",
            f"expected {expected['count']}, found {result['count']!r}",
        )
        require(
            result["ids"] == expected["ids"],
            revision,
            name,
            "ids",
            f"expected {expected['ids']}, found {result['ids']!r}",
        )
        for field in COUNTER_FIELDS:
            require(
                counters[field] == expected[field],
                revision,
                name,
                f"counters.{field}",
                f"expected {expected[field]}, found {counters[field]!r}",
            )

    require(
        set(by_name) == set(EXPECTED),
        revision,
        None,
        "results",
        f"expected {sorted(EXPECTED)}, found {sorted(by_name)}",
    )
    return by_name


def load_json(path, revision):
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise VerificationError(f"{revision}: cannot read {path}: {error}") from error


def validate_trace(revision, document):
    require(
        isinstance(document, dict),
        revision,
        None,
        "source_trace",
        "must be an object",
    )
    require(
        set(document) == set(EXPECTED_TRACE),
        revision,
        None,
        "source_trace",
        f"expected {sorted(EXPECTED_TRACE)}, found {sorted(document)}",
    )
    for query, expected in EXPECTED_TRACE.items():
        require(
            document[query] == expected,
            revision,
            query,
            "source_trace",
            f"expected {expected!r}, found {document[query]!r}",
        )
    return document


def verify_files(a1_path, candidate_path, a1_trace_path, candidate_trace_path):
    a1 = load_json(a1_path, "A1")
    candidate = load_json(candidate_path, "candidate")
    a1_trace = load_json(a1_trace_path, "A1 source trace")
    candidate_trace = load_json(candidate_trace_path, "candidate source trace")
    a1_queries = validate_revision("A1", a1, A1_SHA)
    candidate_queries = validate_revision("candidate", candidate, CANDIDATE_SHA)
    a1_io = validate_trace("A1", a1_trace)
    candidate_io = validate_trace("candidate", candidate_trace)
    require(
        a1_queries == candidate_queries,
        "cross-revision",
        None,
        "results",
        "validated query results differ",
    )
    require(
        a1_io == candidate_io,
        "cross-revision",
        None,
        "source_trace",
        "validated source I/O differs",
    )
    return {
        "a1_sha": A1_SHA,
        "candidate_sha": CANDIDATE_SHA,
        "results_equal": True,
        "source_io_equal": True,
        "results": a1["results"],
        "source_io": a1_io,
    }


def remove_query(document, name):
    index = next(
        index
        for index, result in enumerate(document["results"])
        if result["name"] == name
    )
    document["results"].pop(index)


def duplicate_query(document):
    document["results"][1] = copy.deepcopy(document["results"][0])


def run_self_tests(
    valid_a1,
    valid_candidate,
    valid_a1_trace,
    valid_candidate_trace,
):
    def write_inputs(root, a1, candidate, a1_trace, candidate_trace):
        (root / "a1-reads.json").write_text(json.dumps(a1))
        (root / "candidate-reads.json").write_text(json.dumps(candidate))
        (root / "a1-trace.json").write_text(json.dumps(a1_trace))
        (root / "candidate-trace.json").write_text(json.dumps(candidate_trace))

    def run_cli(root, output_path):
        return subprocess.run(
            [
                sys.executable,
                "-O",
                str(Path(__file__).resolve()),
                "--input-dir",
                str(root),
                "--output",
                str(output_path),
            ],
            capture_output=True,
            text=True,
        )

    def paired_mutation(name, mutate, expected):
        return (name, "both", mutate, expected)

    def candidate_mutation(name, mutate, expected):
        return (name, "candidate", mutate, expected)

    mutations = (
        paired_mutation(
            "paired-empty",
            lambda a1, candidate, a1_trace, candidate_trace: (
                a1.update(results=[]),
                candidate.update(results=[]),
            ),
            "expected 3 entries",
        ),
        paired_mutation(
            "paired-missing",
            lambda a1, candidate, a1_trace, candidate_trace: (
                remove_query(a1, "combined_old"),
                remove_query(candidate, "combined_old"),
            ),
            "expected 3 entries",
        ),
        paired_mutation(
            "paired-duplicate",
            lambda a1, candidate, a1_trace, candidate_trace: (
                duplicate_query(a1),
                duplicate_query(candidate),
            ),
            "duplicate query result",
        ),
        paired_mutation(
            "paired-unknown",
            lambda a1, candidate, a1_trace, candidate_trace: (
                a1["results"][0].update(name="unknown_query"),
                candidate["results"][0].update(name="unknown_query"),
            ),
            "unknown or missing query name",
        ),
        candidate_mutation(
            "one-sided-count",
            lambda a1, candidate, a1_trace, candidate_trace: candidate["results"][0].update(
                count=999
            ),
            "candidate/exact_page/count",
        ),
        candidate_mutation(
            "omitted-counter",
            lambda a1, candidate, a1_trace, candidate_trace: candidate["results"][0][
                "counters"
            ].pop("bytes_read"),
            "candidate/exact_page/counters.fields",
        ),
        candidate_mutation(
            "reordered-ids",
            lambda a1, candidate, a1_trace, candidate_trace: next(
                result for result in candidate["results"] if result["name"] == "combined_old"
            )["ids"].reverse(),
            "candidate/combined_old/ids",
        ),
        candidate_mutation(
            "altered-trace-offset",
            lambda a1, candidate, a1_trace, candidate_trace: candidate_trace[
                "combined_old"
            ]["reads"][0].update(offset=1),
            "candidate/combined_old/source_trace",
        ),
    )

    with tempfile.TemporaryDirectory(prefix="objekte-verify-valid-") as directory:
        root = Path(directory)
        write_inputs(
            root,
            valid_a1,
            valid_candidate,
            valid_a1_trace,
            valid_candidate_trace,
        )
        output_path = root / "valid-comparison.json"
        completed = run_cli(root, output_path)
        if completed.returncode != 0:
            raise VerificationError(
                f"self-test valid control failed: {completed.stderr}"
            )
        if not output_path.exists():
            raise VerificationError("self-test valid control wrote no comparison")

    for name, target, mutate, expected in mutations:
        with tempfile.TemporaryDirectory(prefix=f"objekte-verify-{name}-") as directory:
            root = Path(directory)
            a1 = copy.deepcopy(valid_a1)
            candidate = copy.deepcopy(valid_candidate)
            a1_trace = copy.deepcopy(valid_a1_trace)
            candidate_trace = copy.deepcopy(valid_candidate_trace)
            if target == "both":
                mutate(a1, candidate, a1_trace, candidate_trace)
            else:
                mutate(a1, candidate, a1_trace, candidate_trace)
            write_inputs(root, a1, candidate, a1_trace, candidate_trace)
            output_path = root / "must-not-exist.json"
            completed = run_cli(root, output_path)
            if completed.returncode == 0:
                raise VerificationError(
                    f"self-test {name}: invalid input exited successfully"
                )
            if output_path.exists():
                raise VerificationError(
                    f"self-test {name}: invalid input wrote a success result"
                )
            if expected not in completed.stderr:
                raise VerificationError(
                    f"self-test {name}: expected {expected!r}, got {completed.stderr!r}"
                )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--input-dir",
        type=Path,
        default=Path(__file__).resolve().parent,
        help="directory containing a1-reads.json and candidate-reads.json",
    )
    parser.add_argument("--output", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    a1_path = args.input_dir / "a1-reads.json"
    candidate_path = args.input_dir / "candidate-reads.json"
    a1_trace_path = args.input_dir / "a1-trace.json"
    candidate_trace_path = args.input_dir / "candidate-trace.json"
    output_path = args.output or args.input_dir / "comparison.json"
    a1 = load_json(a1_path, "A1")
    candidate = load_json(candidate_path, "candidate")
    a1_trace = load_json(a1_trace_path, "A1 source trace")
    candidate_trace = load_json(candidate_trace_path, "candidate source trace")
    if args.self_test:
        run_self_tests(a1, candidate, a1_trace, candidate_trace)
    comparison = verify_files(
        a1_path,
        candidate_path,
        a1_trace_path,
        candidate_trace_path,
    )
    output_path.write_text(json.dumps(comparison, separators=(",", ":")) + "\n")
    print(json.dumps(comparison, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except VerificationError as error:
        print(f"verification failed: {error}", file=sys.stderr)
        raise SystemExit(1)
