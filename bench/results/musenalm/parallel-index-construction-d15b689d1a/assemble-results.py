#!/usr/bin/env python3
"""Validate and print the saved parallel construction comparison tables."""

import csv
import json
import sys
from pathlib import Path


HERE = Path(__file__).resolve().parent
BASELINE = HERE.parent / "parallel-bigram-4f2cb8e85b" / "gate.json"
sys.path.insert(0, str(HERE.parents[3] / "bench"))
from schema_evidence import canonical_registrations


def gate(path: Path) -> dict:
    value = json.loads(path.read_text())
    if value["failures"]:
        raise ValueError(f"failed build gate: {path}")
    return value


def main() -> None:
    old = gate(BASELINE)
    one = gate(HERE / "one/gate.json")
    auto = gate(HERE / "auto/gate.json")
    measured = "d15b689d1ac21709195b79fc382c045d8dec9867"
    for current in (one, auto):
        if current["environment"]["revision"] != measured:
            raise ValueError("wrong measured code revision")
        if current["fingerprints"] != old["fingerprints"]:
            raise ValueError("corpus fingerprint differs from serial baseline")
        if current["schema_manifest_digest"] != old["schema_manifest_digest"]:
            raise ValueError("semantic schema differs from serial baseline")
        if canonical_registrations(current) != canonical_registrations(old):
            raise ValueError("registrations differ from serial baseline")
        for field in ("cpu_model", "rustc", "affinity", "governor", "build_flags"):
            if current["environment"][field] != old["environment"][field]:
                raise ValueError(f"environment differs from serial baseline: {field}")
    rows = []
    for name in old["summary"]:
        baseline = old["summary"][name]
        serial = one["summary"][name]
        parallel = auto["summary"][name]
        if serial["index_signature_sha256"] != parallel["index_signature_sha256"]:
            raise ValueError(f"semantic index mismatch: {name}")
        before = baseline["median_wall_ns"] / 1e6
        one_ms = serial["median_wall_ns"] / 1e6
        auto_ms = parallel["median_wall_ns"] / 1e6
        rows.append({
            "configuration": name,
            "baseline_serial_ms": before,
            "new_one_worker_ms": one_ms,
            "new_auto_ms": auto_ms,
            "one_vs_baseline_ms": one_ms - before,
            "one_vs_baseline_percent": 100 * (one_ms / before - 1),
            "auto_vs_baseline_ms": auto_ms - before,
            "auto_vs_baseline_percent": 100 * (auto_ms / before - 1),
            "auto_vs_one_ms": auto_ms - one_ms,
            "auto_peak_rss_mib": parallel["median_peak_resident_bytes"] / 1048576,
            "index_signature_sha256": parallel["index_signature_sha256"],
        })
    with (HERE / "comparison.csv").open("w", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=rows[0].keys())
        writer.writeheader()
        writer.writerows(rows)
    (HERE / "comparison.json").write_text(json.dumps({"measured_sha": measured, "rows": rows}, indent=2) + "\n")
    print("| Configuration | Baseline ms | New one ms | New auto ms | Auto − baseline ms | Auto peak RSS MiB |")
    print("|---|---:|---:|---:|---:|---:|")
    for row in rows:
        print(f"| {row['configuration']} | {row['baseline_serial_ms']:.1f} | "
              f"{row['new_one_worker_ms']:.1f} | {row['new_auto_ms']:.1f} | "
              f"{row['auto_vs_baseline_ms']:+.1f} | {row['auto_peak_rss_mib']:.1f} |")
    print("\n| Alphabet | Workers | Median ms | Min–max ms | CPU s | Peak RSS MiB | Same index |")
    print("|---|---:|---:|---:|---:|---:|---|")
    for alphabet in ("ascii", "unicode"):
        reference = one["summary"][f"{alphabet}-disabled"]["index_signature_sha256"]
        for workers in ("one", "w2", "w4", "w8", "w16", "auto"):
            row = gate(HERE / workers / "gate.json")["summary"][f"{alphabet}-disabled"]
            equivalent = row["index_signature_sha256"] == reference
            if not equivalent:
                raise ValueError(f"semantic index mismatch: {alphabet} {workers}")
            print(f"| {alphabet} | {row['resolved_build_workers']} | {row['median_wall_ns']/1e6:.1f} | "
                  f"{row['min_wall_ns']/1e6:.1f}–{row['max_wall_ns']/1e6:.1f} | "
                  f"{row['median_process_user_seconds'] + row['median_process_system_seconds']:.2f} | "
                  f"{row['median_peak_resident_bytes']/1048576:.1f} | yes |")


if __name__ == "__main__":
    main()
