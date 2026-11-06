#!/usr/bin/env python3
"""Summarize result-equivalent, schema enabled old/new core timings."""

from __future__ import annotations

import argparse
import csv
import json
import statistics
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, help="existing JSONL; omit to run the comparison")
    parser.add_argument("--result-dir", type=Path, required=True)
    args = parser.parse_args()
    result_dir = args.result_dir.resolve()
    result_dir.mkdir(parents=True, exist_ok=True)
    input_path = args.input or result_dir / "raw.jsonl"
    if args.input is None:
        command = ["cargo", "run", "--release", "--manifest-path", "crates/old-core-compare/Cargo.toml", "--", "--repetitions", "5"]
        with input_path.open("w") as output, (result_dir / "console.log").open("w") as log:
            completed = subprocess.run(command, cwd=ROOT, stdout=output, stderr=log)
        if completed.returncode:
            raise RuntimeError(f"comparison failed: {result_dir / 'console.log'}")
    entries = [json.loads(line) for line in input_path.read_text().splitlines()]
    if len([entry for entry in entries if entry["type"] == "setup"]) != 1:
        raise ValueError("expected one setup record")
    setup = entries[0]
    if setup["old_schema_mode"] != "configured" or setup["new_schema_mode"] != "configured":
        raise ValueError("schema enabled setup required for both cores")
    if setup.get("indexed_path_sets_equal") is not True:
        raise ValueError("the old and new indexed path sets must match exactly")
    cases = [entry for entry in entries if entry["type"] == "case"]
    skipped = [entry for entry in entries if entry["type"] == "skip"]
    if not cases:
        raise ValueError("no result-equivalent cases")
    if len(cases) + len(skipped) != 34:
        raise ValueError(f"expected 34 shared cases, found {len(cases) + len(skipped)}")
    if input_path.resolve() != (result_dir / "raw.jsonl").resolve():
        (result_dir / "raw.jsonl").write_text(input_path.read_text())
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    with (result_dir / "timings.csv").open("w", newline="") as file:
        output = csv.writer(file)
        output.writerow(("case", "collection", "matches", "old_median_ms", "new_median_ms", "old_over_new", "old_source_reads", "new_files_opened", "old_parses", "new_parses"))
        for entry in cases:
            old = entry["old_median_ns"]
            new = entry["new_median_ns"]
            output.writerow((entry["case"], entry["collection"], entry["matches"], old / 1e6, new / 1e6, old / new, entry.get("old_source_reads"), entry.get("new_files_opened"), entry.get("old_parses"), entry.get("new_parses")))
    ratios = [entry["old_median_ns"] / entry["new_median_ns"] for entry in cases]
    result = {
        "revision": revision, "setup": setup, "compared": len(cases), "skipped": len(skipped),
        "median_old_over_new": statistics.median(ratios),
        "old_faster": sum(ratio < 1 for ratio in ratios),
        "new_faster": sum(ratio > 1 for ratio in ratios),
        "skipped_cases": [{"case": row["case"], "reason": row["reason"], "old_count": row.get("old_count"), "new_count": row.get("new_count")} for row in skipped],
    }
    (result_dir / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
    lines = [
        "# Archived core versus new search core", "",
        f"Revision: `{revision}`. Result-equivalent cases: {len(cases)}; skipped: {len(skipped)}.",
        f"Median old/new ratio: {statistics.median(ratios):.2f}×; old faster: {result['old_faster']}; new faster: {result['new_faster']}.",
        "", "Both cores load the Musenalm collection schemas and use ASCII bigrams.",
        "The old core loads its configured schemas and warms every bigram index before timing.",
        "The new core loads its schema definitions and restores or builds its index before timing.",
        f"Loaded collections: {setup['old_collections']} old, {setup['new_collections']} new; old schema files: {setup['old_schemas']}.",
        f"The configured indexed path sets match exactly: {setup['old_index_paths']} paths in each core.",
        "Both have verified query caches disabled, return an exact count and the first 50 full logical records,",
        "and use record ID ascending order. Cases enter timing only after exact count and ordered page IDs match.",
        "One process runs both cores, alternating their order across five repetitions per case.",
        "The archived core uses its Memory repository mode for indexed queries; the new core has no source-content cache.",
        "Per-core parsing and materialization costs remain implementation-specific.",
        "", "| Case | Matches | Old ms | New ms | Old / new | Old reads | New opens |", "|---|---:|---:|---:|---:|---:|---:|",
    ]
    for entry in cases:
        old = entry["old_median_ns"] / 1e6
        new = entry["new_median_ns"] / 1e6
        lines.append(f"| {entry['case']} | {entry['matches']} | {old:.3f} | {new:.3f} | {old / new:.2f}× | {entry.get('old_source_reads', '?')} | {entry.get('new_files_opened', '?')} |")
    if skipped:
        lines += ["", "## Excluded before timing", ""]
        for entry in skipped:
            lines.append(f"- {entry['case']}: {entry['reason']} (old={entry.get('old_count')}, new={entry.get('new_count')})")
    lines += ["", "Raw paired samples: [raw.jsonl](raw.jsonl); machine readable summary: [summary.json](summary.json).", ""]
    (result_dir / "REPORT.md").write_text("\n".join(lines))
    print(f"{len(cases)} matched cases, {len(skipped)} excluded; median old/new {statistics.median(ratios):.2f}×")
    print(result_dir)
    return int(bool(skipped))


if __name__ == "__main__":
    sys.exit(main())
