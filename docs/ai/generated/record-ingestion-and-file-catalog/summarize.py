#!/usr/bin/env python3
"""Validate the final evidence and write its side-by-side comparison."""

import csv
import hashlib
import json
from pathlib import Path
import statistics


HERE = Path(__file__).resolve().parent
BEFORE = HERE / "before"
AFTER = HERE / "after"
CONFIRM = HERE / "confirmation"


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def source_rows(phase, side):
    path = (BEFORE if side == "before" else AFTER) / phase / "results/query-results.csv"
    return {row["case_id"]: row for row in csv.DictReader(path.open())}


def first_samples(phase, side):
    path = (BEFORE if side == "before" else AFTER) / phase / "results/measurements.jsonl"
    return {
        row["case_id"]: row["total_ns"]
        for row in (json.loads(line) for line in path.read_text().splitlines())
        if row["repetition"] == 0
    }


def lifecycle_rows(side, parent=None):
    root = parent or (BEFORE if side == "before" else AFTER)
    rows = []
    for sample in (1, 2, 3):
        path = root / f"lifecycle-small-{sample}" / "stdout.log"
        rows.append({row["phase"]: row for row in csv.DictReader(path.open(), delimiter="\t")})
    return rows


def signed(before, after):
    return after - before, None if before == 0 else (after / before - 1) * 100


def main():
    base = json.loads((BEFORE / "manifest.json").read_text())
    final = json.loads((AFTER / "manifest.json").read_text())
    for key in (
        "fixture", "fixture_fingerprint", "cargo_lock_sha256",
        "historical_manifest_sha256", "extended_manifest_sha256", "rustc", "cargo",
        "affinity", "governors", "workers", "query_workers", "rustflags", "allocator",
    ):
        assert base[key] == final[key], key
    assert len(final["invocations"]) == 14
    assert all(entry["exit_status"] == 0 for entry in final["invocations"])
    confirmation = json.loads((CONFIRM / "manifest.json").read_text())
    assert confirmation["sides"]["after"]["sha"] == final["base_commit"]
    assert all(entry["exit_status"] == 0 for entry in confirmation["invocations"])
    canonical = {}
    for name in ("ascii", "unicode", "extended-unicode"):
        previous = json.loads((BEFORE / f"canonical-{name}.json").read_text())
        current = json.loads((AFTER / f"canonical-{name}.json").read_text())
        assert previous["cases"] == current["cases"], name
        canonical[name] = current["semantic_sha256"]
    probe_before = json.loads((BEFORE / "direct-probe.json").read_text())
    probe_after = json.loads((AFTER / "direct-probe.json").read_text())
    assert probe_before["exact_count"] == probe_after["exact_count"] == 1024
    assert probe_before["residual_count"] == probe_after["residual_count"] == 1024
    assert probe_after["exact_counters"]["files_opened"] == 0
    assert probe_after["exact_counters"]["bytes_read"] == 0
    native = json.loads((AFTER / "native/manifest.json").read_text())
    assert native["implementation_sha"] == final["base_commit"]
    assert len(native["runs"]) == 27
    assert all(run["exit_status"] == 0 for run in native["runs"])
    assert (AFTER / "native/completed").exists()

    comparison = []

    def row(metric, configuration, before, after, unit, gate, source):
        delta, percent = signed(before, after)
        comparison.append({
            "metric": metric, "configuration_or_qid": configuration,
            "before": before, "after": after, "delta": delta,
            "delta_pct": "" if percent is None else round(percent, 3),
            "unit": unit, "verdict": gate, "source": source,
        })

    for alphabet in ("ascii", "unicode"):
        builds = {}
        for side, root in (("before", BEFORE), ("after", AFTER)):
            builds[side] = [
                json.loads((root / f"build-{alphabet}-{sample}/stdout.log").read_text())
                for sample in (1, 2, 3)
            ]
        for metric, unit in (("wall_ns", "ns"), ("peak_resident_bytes", "bytes"),
                             ("resident_bytes", "bytes"), ("index_family_bytes", "bytes")):
            previous = statistics.median(run[metric] for run in builds["before"])
            current = statistics.median(run[metric] for run in builds["after"])
            gate = "pass" if metric not in ("wall_ns", "peak_resident_bytes") or current <= 1.1 * previous else "confirm"
            row(f"build_{metric}", alphabet, previous, current, unit, gate, "three process medians")
        for key in ("records", "build_parses", "failed_parses", "index_stores"):
            assert [run[key] for run in builds["before"]] == [run[key] for run in builds["after"]], (alphabet, key)
        for side in ("before", "after"):
            for run in builds[side]:
                assert {
                    collection: (len(segments), sum(segment[1] for segment in segments),
                                 sum(segment[2] for segment in segments))
                    for collection, segments in run["segment_counts"].items()
                } == {
                    collection: (len(segments), sum(segment[1] for segment in segments),
                                 sum(segment[2] for segment in segments))
                    for collection, segments in builds["before"][0]["segment_counts"].items()
                }, (alphabet, side, "segment counts")

    query_counts = {}
    for phase, expected in (("historical-ascii", 150), ("historical-unicode", 150),
                            ("focused-ascii", 9), ("focused-unicode", 9),
                            ("extended-unicode", 6)):
        old, new = source_rows(phase, "before"), source_rows(phase, "after")
        assert len(old) == len(new) == expected, phase
        query_counts[phase] = expected
        for qid in old:
            for field in ("status", "total_matches", "returned_records", "candidates",
                          "files_opened", "bytes_read", "record_parses"):
                assert old[qid][field] == new[qid][field], (phase, qid, field)
        if phase.startswith("focused") or phase.startswith("extended"):
            first_old = first_samples(phase, "before")
            first_new = first_samples(phase, "after")
            for qid in old:
                previous, current = int(old[qid]["median_ns"]), int(new[qid]["median_ns"])
                breach = current > 1.1 * previous and current - previous > 5000
                row("query_median", f"{phase}/{qid}", previous, current, "ns",
                    "confirm" if breach else "pass", "seven samples per process")
                row("query_first", f"{phase}/{qid}", first_old[qid],
                    first_new[qid], "ns", "informational", "first measured sample")
            if phase.startswith("focused"):
                previous = sum(int(entry["median_ns"]) for entry in old.values())
                current = sum(int(entry["median_ns"]) for entry in new.values())
                row("focused_median_sum", phase, previous, current, "ns",
                    "confirm" if current > 1.1 * previous else "pass", "nine query medians")

    lookup_before = statistics.median(probe_before["lookup_samples_ns"]) / 10000
    lookup_after = statistics.median(probe_after["lookup_samples_ns"]) / 10000
    row("id_lookup_per_operation", "16384 files", lookup_before, lookup_after, "ns",
        "confirm" if lookup_after > 1.1 * lookup_before and lookup_after - lookup_before > 5000 else "pass",
        "seven 10000-operation samples")
    row("residual_query_median", "year = 1808 && note ~ Leipzig",
        statistics.median(probe_before["residual_samples_ns"]),
        statistics.median(probe_after["residual_samples_ns"]), "ns", "pass", "seven samples")

    lifecycle = {}
    for side in ("before", "after"):
        sample_rows = lifecycle_rows(side)
        lifecycle[side] = {phase: statistics.median(float(sample[phase]["ms"]) for sample in sample_rows)
                           for phase in sample_rows[0]}
    for phase, previous in lifecycle["before"].items():
        current = lifecycle["after"][phase]
        breach = previous > 0 and current > 1.1 * previous and current - previous > 0.005
        row("lifecycle_phase", phase, previous, current, "ms",
            "confirm" if breach else "pass", "three process medians")

    confirmed = {}
    for phase in ("historical-ascii", "historical-unicode", "lifecycle-small"):
        paired = {}
        for side in ("before", "after"):
            observations = []
            for pair in (1, 2, 3):
                stem = CONFIRM / f"{phase}-pair-{pair}-{side}"
                if phase.startswith("historical"):
                    observations.append({entry["case_id"]: entry for entry in
                                         csv.DictReader((stem / "results/query-results.csv").open())})
                else:
                    observations.append({entry["phase"]: entry for entry in
                                         csv.DictReader((stem / "stdout.log").open(), delimiter="\t")})
            paired[side] = observations
        keys = paired["before"][0]
        for key in keys:
            field = "median_ns" if phase.startswith("historical") else "ms"
            converter = int if phase.startswith("historical") else float
            previous = statistics.median(converter(obs[key][field]) for obs in paired["before"])
            current = statistics.median(converter(obs[key][field]) for obs in paired["after"])
            floor = 5000 if phase.startswith("historical") else 0.005
            breach = previous > 0 and current > 1.1 * previous and current - previous > floor
            row("confirmed_query" if phase.startswith("historical") else "confirmed_lifecycle",
                f"{phase}/{key}", previous, current,
                "ns" if phase.startswith("historical") else "ms",
                "breach" if breach else "pass", "three interleaved process medians")
            confirmed[f"{phase}/{key}"] = "breach" if breach else "pass"

    for variant in sorted({"-".join(run["label"].split("-")[:-1]) for run in native["runs"]}):
        samples = [run["observation"] for run in native["runs"] if run["label"].startswith(variant + "-")]
        fields = ("producer_accept_ns", "publish_drain_ns", "high_water_ram_bytes",
                  "high_water_spool_bytes", "spool_written_bytes", "spool_read_bytes",
                  "peak_rss_kib", "index_bytes", "segments") if variant.startswith("ingest") else (
                      "lookup_ns", "reverse_ns", "route_segment_visits", "reverse_members",
                      "peak_rss_kib", "index_bytes", "segments")
        for field in fields:
            current = statistics.median(sample[field] for sample in samples)
            comparison.append({
                "metric": f"native_{field}", "configuration_or_qid": variant,
                "before": "N/A — API absent", "after": current, "delta": "",
                "delta_pct": "", "unit": "ns" if field.endswith("_ns") else "count/bytes",
                "verdict": "pass", "source": "three fresh after-only processes",
            })

    with (HERE / "comparison.csv").open("w", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=list(comparison[0]), lineterminator="\n")
        writer.writeheader()
        writer.writerows(comparison)
    validation = {
        "source_commit": final["base_commit"],
        "all_required_invocations_passed": True,
        "matching_frozen_conditions": True,
        "query_rows": query_counts,
        "canonical_sha256": canonical,
        "probe_binary_sha256": digest(Path(__import__("sys").argv[1])),
        "probe_fixture": str(Path("/home/simon/source/objekte-record-ingestion-probe-fixture")),
        "probe_exact_count_zero_source_reads": True,
        "native_runs": len(native["runs"]),
        "confirmed_gates": confirmed,
    }
    (AFTER / "validation.json").write_text(json.dumps(validation, indent=2) + "\n")
    checksums = []
    for path in sorted(AFTER.rglob("*")):
        if path.is_file() and path.name != "checksums.sha256":
            checksums.append(f"{digest(path)}  {path.relative_to(AFTER)}")
    (AFTER / "checksums.sha256").write_text("\n".join(checksums) + "\n")
    print(json.dumps({"rows": len(comparison), "breaches": [key for key, verdict in confirmed.items() if verdict == "breach"]}))


if __name__ == "__main__":
    main()
