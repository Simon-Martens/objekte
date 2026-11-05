#!/usr/bin/env python3
"""Produce the per-query and profile tables from the saved raw measurements."""

import argparse
import json
from pathlib import Path
from statistics import median


def query_rows(root: Path) -> list[dict]:
    summary = json.loads((root / "queries/summary.json").read_text())
    sets = {}
    for budget in (0, 64):
        name = f"json-schema-ascii-cache{budget}"
        folder = root / "queries" / name
        master = [json.loads(line) for line in
                  (folder / "01-reference.stdout.ndjson").read_text().splitlines()]
        current = [json.loads(line) for line in
                   (folder / "01-candidate.stdout.ndjson").read_text().splitlines()]
        assert len(master) == len(current) == 153
        for a, b in zip(master[1:], current[1:]):
            assert (a["id"], a["filter"], a["matches"], a["first_id"]) == (
                b["id"], b["filter"], b["matches"], b["first_id"])
        sets[budget] = (master, current, summary[name]["cases"])
    result = []
    for index in range(1, 153):
        base = sets[0][0][index]
        other = sets[64][0][index]
        assert (base["filter"], base["matches"], base["first_id"]) == (
            other["filter"], other["matches"], other["first_id"])
        row = {"id": index, "filter": base["filter"], "matches": base["matches"],
               "first_id": base["first_id"]}
        for budget in (0, 64):
            case = sets[budget][2][index - 1]
            times = case["median_ms"]
            master_ms = times["reference_median"]
            current_ms = times["candidate_median"]
            row[f"cache{budget}"] = {
                "master_median_ms": master_ms,
                "current_median_ms": current_ms,
                "delta_ms": current_ms - master_ms,
                "delta_percent": (current_ms / master_ms - 1) * 100,
                "work_equal": all(v for k, v in case.items() if k.endswith("_equal")),
            }
        result.append(row)
    return result


def profile_rows(root: Path, operation: str) -> list[dict]:
    field = "build_wall_nanos" if operation == "build" else "snapshot_restore_wall_nanos"
    data = [json.loads(line) for line in (root / "index" / f"{operation}.ndjson").read_text().splitlines()]
    assert len(data) == 100
    rows = []
    for name in sorted({row["profile"] for row in data}):
        selected = [row for row in data if row["profile"] == name]
        master = [row[field] / 1e6 for row in selected if row["revision"] == "master"]
        current = [row[field] / 1e6 for row in selected if row["revision"] == "current"]
        assert len(master) == len(current) == 5
        stable = ("documents", "compressed_index_bytes", "index_sizes")
        if operation == "restore":
            stable += ("snapshot_base_bytes", "snapshot_secondary_bytes")
        mismatches = [key for key in stable if len({json.dumps(row.get(key), sort_keys=True)
                                                  for row in selected}) != 1]
        master_med, current_med = median(master), median(current)
        rows.append({
            "profile": name, "documents": selected[0]["documents"],
            "compressed_index_bytes": selected[0]["compressed_index_bytes"],
            "master_samples_ms": master, "current_samples_ms": current,
            "master_median_ms": master_med, "current_median_ms": current_med,
            "delta_ms": current_med - master_med,
            "delta_percent": (current_med / master_med - 1) * 100,
            "signature_mismatches": mismatches,
        })
    return rows


def query_table(rows: list[dict]) -> str:
    lines = [
        "| # | Query | Reported total (cap 10,000) | First ID | Master 0 MiB (ms) | Current 0 MiB (ms) | Δ 0 MiB | Master 64 MiB (ms) | Current 64 MiB (ms) | Δ 64 MiB |",
        "| ---: | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for row in rows:
        values = []
        for budget in (0, 64):
            part = row[f"cache{budget}"]
            values.extend((f"{part['master_median_ms']:.3f}",
                           f"{part['current_median_ms']:.3f}",
                           f"{part['delta_ms']:+.3f} ({part['delta_percent']:+.1f}%)"))
        query = row["filter"].replace("|", "\\|")
        first_id = row["first_id"] or "—"
        lines.append(f"| {row['id']} | `{query}` | {row['matches']:,} | {first_id} | "
                     + " | ".join(values) + " |")
    return "\n".join(lines) + "\n"


def profile_table(rows: list[dict]) -> str:
    lines = [
        "| Profile | Records | Master median (ms) | Current median (ms) | Δ ms | Δ % |",
        "| --- | ---: | ---: | ---: | ---: | ---: |",
    ]
    for row in rows:
        lines.append(
            f"| {row['profile']} | {row['documents']:,} | "
            f"{row['master_median_ms']:.3f} | {row['current_median_ms']:.3f} | "
            f"{row['delta_ms']:+.3f} | {row['delta_percent']:+.2f}% |"
        )
    return "\n".join(lines) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    root, output = args.root.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    queries = query_rows(root)
    builds = profile_rows(root, "build")
    restores = profile_rows(root, "restore")
    (output / "summary.json").write_text(json.dumps({
        "queries": queries, "builds": builds, "restores": restores,
    }, indent=2, sort_keys=True) + "\n")
    (output / "query-table.md").write_text(query_table(queries))
    (output / "build-table.md").write_text(profile_table(builds))
    (output / "restore-table.md").write_text(profile_table(restores))


if __name__ == "__main__":
    main()
