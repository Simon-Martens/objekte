#!/usr/bin/env python3
"""Generate catalog-led storage summaries from the raw benchmark TSV files."""

from __future__ import annotations

import csv
import math
import statistics
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parent
FINAL = ROOT / "final"


def read_samples(name: str) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    for alphabet_dir in sorted(path for path in FINAL.iterdir() if path.is_dir()):
        path = alphabet_dir / name
        if not path.is_file():
            continue
        with path.open(newline="") as handle:
            rows.extend(csv.DictReader(handle, delimiter="\t"))
    return rows


def write_rows(name: str, fieldnames: list[str], rows: list[dict[str, object]]) -> None:
    with (ROOT / name).open("w", newline="") as handle:
        writer = csv.DictWriter(
            handle,
            fieldnames=fieldnames,
            delimiter="\t",
            lineterminator="\n",
        )
        writer.writeheader()
        writer.writerows(rows)


def percentile(values: list[int], fraction: float) -> int:
    ordered = sorted(values)
    index = max(0, math.ceil(len(ordered) * fraction) - 1)
    return ordered[index]


def summarize_numeric(
    rows: list[dict[str, str]],
    keys: list[str],
    metrics: list[str],
) -> list[dict[str, object]]:
    groups: dict[tuple[str, ...], list[dict[str, str]]] = defaultdict(list)
    for row in rows:
        groups[tuple(row[key] for key in keys)].append(row)
    output: list[dict[str, object]] = []
    for group, samples in sorted(groups.items()):
        result: dict[str, object] = dict(zip(keys, group))
        result["samples"] = len(samples)
        for metric in metrics:
            values = [int(sample[metric]) for sample in samples]
            result[f"{metric}_median"] = round(statistics.median(values))
            result[f"{metric}_p95"] = percentile(values, 0.95)
            result[f"{metric}_max"] = max(values)
        output.append(result)
    return output


def main() -> None:
    construction = read_samples("construction-samples.tsv")
    write_rows(
        "construction-samples.tsv",
        [
            "alphabet",
            "workers",
            "entry_point",
            "elapsed_ns",
            "rss_bytes",
            "peak_rss_bytes",
            "rss_before_bytes",
        ],
        construction,
    )
    write_rows(
        "construction-summary.tsv",
        [
            "alphabet",
            "workers",
            "entry_point",
            "samples",
            "elapsed_ns_median",
            "elapsed_ns_p95",
            "elapsed_ns_max",
            "rss_bytes_median",
            "rss_bytes_p95",
            "rss_bytes_max",
            "peak_rss_bytes_median",
            "peak_rss_bytes_p95",
            "peak_rss_bytes_max",
            "rss_before_bytes_median",
            "rss_before_bytes_p95",
            "rss_before_bytes_max",
        ],
        summarize_numeric(
            construction,
            ["alphabet", "workers", "entry_point"],
            ["elapsed_ns", "rss_bytes", "peak_rss_bytes", "rss_before_bytes"],
        ),
    )

    save = read_samples("save-samples.tsv")
    write_rows(
        "save-samples.tsv",
        [
            "alphabet",
            "variant",
            "elapsed_ns",
            "bytes_serialized",
            "bytes_written",
            "objects_new",
            "objects_reused",
            "sync_calls",
            "rss_bytes",
            "peak_rss_bytes",
        ],
        save,
    )
    write_rows(
        "save-summary.tsv",
        [
            "alphabet",
            "variant",
            "samples",
            "elapsed_ns_median",
            "elapsed_ns_p95",
            "elapsed_ns_max",
            "bytes_serialized_median",
            "bytes_serialized_p95",
            "bytes_serialized_max",
            "bytes_written_median",
            "bytes_written_p95",
            "bytes_written_max",
            "objects_new_median",
            "objects_new_p95",
            "objects_new_max",
            "objects_reused_median",
            "objects_reused_p95",
            "objects_reused_max",
            "sync_calls_median",
            "sync_calls_p95",
            "sync_calls_max",
            "rss_bytes_median",
            "rss_bytes_p95",
            "rss_bytes_max",
            "peak_rss_bytes_median",
            "peak_rss_bytes_p95",
            "peak_rss_bytes_max",
        ],
        summarize_numeric(
            save,
            ["alphabet", "variant"],
            [
                "elapsed_ns",
                "bytes_serialized",
                "bytes_written",
                "objects_new",
                "objects_reused",
                "sync_calls",
                "rss_bytes",
                "peak_rss_bytes",
            ],
        ),
    )

    opens = read_samples("open-samples.tsv")
    write_rows(
        "open-samples.tsv",
        [
            "alphabet",
            "sample",
            "elapsed_ns",
            "index_reads",
            "index_bytes",
            "file_rows",
            "record_rows",
            "index_descriptors",
            "leaf_rows",
            "rss_bytes",
            "peak_rss_bytes",
        ],
        opens,
    )
    write_rows(
        "open-summary.tsv",
        [
            "alphabet",
            "samples",
            "elapsed_ns_median",
            "elapsed_ns_p95",
            "elapsed_ns_max",
            "index_reads_median",
            "index_reads_p95",
            "index_reads_max",
            "index_bytes_median",
            "index_bytes_p95",
            "index_bytes_max",
            "file_rows_median",
            "file_rows_p95",
            "file_rows_max",
            "record_rows_median",
            "record_rows_p95",
            "record_rows_max",
            "index_descriptors_median",
            "index_descriptors_p95",
            "index_descriptors_max",
            "leaf_rows_median",
            "leaf_rows_p95",
            "leaf_rows_max",
            "rss_bytes_median",
            "rss_bytes_p95",
            "rss_bytes_max",
            "peak_rss_bytes_median",
            "peak_rss_bytes_p95",
            "peak_rss_bytes_max",
        ],
        summarize_numeric(
            opens,
            ["alphabet"],
            [
                "elapsed_ns",
                "index_reads",
                "index_bytes",
                "file_rows",
                "record_rows",
                "index_descriptors",
                "leaf_rows",
                "rss_bytes",
                "peak_rss_bytes",
            ],
        ),
    )

    queries = read_samples("query-samples.tsv")
    native = {
        (row["alphabet"], row["case_id"]): (row["result_digest"], row["count"])
        for row in queries
        if row["state"] == "native"
    }
    mismatches = [
        row
        for row in queries
        if row["state"] != "native"
        and native[(row["alphabet"], row["case_id"])]
        != (row["result_digest"], row["count"])
    ]
    if mismatches:
        raise SystemExit(f"{len(mismatches)} storage query results differ from native")
    write_rows(
        "query-samples.tsv",
        [
            "alphabet",
            "state",
            "case_id",
            "repetition",
            "elapsed_ns",
            "count",
            "returned",
            "result_digest",
            "index_reads",
            "index_bytes",
        ],
        queries,
    )
    write_rows(
        "query-summary.tsv",
        [
            "alphabet",
            "state",
            "samples",
            "elapsed_ns_median",
            "elapsed_ns_p95",
            "elapsed_ns_max",
            "index_reads_median",
            "index_reads_p95",
            "index_reads_max",
            "index_bytes_median",
            "index_bytes_p95",
            "index_bytes_max",
        ],
        summarize_numeric(
            queries,
            ["alphabet", "state"],
            ["elapsed_ns", "index_reads", "index_bytes"],
        ),
    )

    for name in [
        "cache-sequence.tsv",
        "storage-io-counters.tsv",
        "memory-summary.tsv",
        "object-size-summary.tsv",
    ]:
        rows = read_samples(name)
        fieldnames = list(rows[0]) if rows else []
        write_rows(name, fieldnames, rows)


if __name__ == "__main__":
    main()
