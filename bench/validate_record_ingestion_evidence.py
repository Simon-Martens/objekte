#!/usr/bin/env python3
"""Check one frozen measurement phase and write its compact evidence manifest."""

import argparse
import csv
import hashlib
import json
from pathlib import Path


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("phase", type=Path)
    parser.add_argument("--probe-fixture", type=Path, required=True)
    parser.add_argument("--probe-binary", type=Path, required=True)
    args = parser.parse_args()
    phase = args.phase
    manifest = json.loads((phase / "manifest.json").read_text())
    assert (phase / "completed").is_file()
    assert len(manifest["invocations"]) == 14
    assert all(item["exit_status"] == 0 for item in manifest["invocations"])
    query_rows = {}
    for label, count, samples in (("historical-ascii", 150, 1),
                                  ("historical-unicode", 150, 1),
                                  ("focused-ascii", 9, 7),
                                  ("focused-unicode", 9, 7),
                                  ("extended-unicode", 6, 7)):
        with (phase / label / "results/query-results.csv").open() as stream:
            rows = list(csv.DictReader(stream))
        assert len(rows) == count, (label, len(rows))
        assert all(row["status"] == "ok" and int(row["samples"]) == samples for row in rows), label
        query_rows[label] = {row["case_id"]: int(row["total_matches"]) for row in rows}
    assert query_rows["historical-ascii"] == query_rows["historical-unicode"]
    for alphabet in ("ascii", "unicode"):
        for sample in range(1, 4):
            build = json.loads((phase / f"build-{alphabet}-{sample}/stdout.log").read_text())
            assert build["records"] == 148_486 and build["failed_parses"] == 0
    canonical = {}
    for label, count in (("ascii", 150), ("unicode", 150),
                         ("extended-unicode", 6)):
        data = json.loads((phase / f"canonical-{label}.json").read_text())
        assert len(data["cases"]) == count, label
        canonical[label] = data["semantic_sha256"]
    assert canonical["ascii"] == canonical["unicode"]
    probe = json.loads((phase / "direct-probe.json").read_text())
    assert probe["exact_count"] == 1024 and probe["residual_count"] == 1024
    assert probe["exact_counters"]["files_opened"] == 0
    assert probe["exact_counters"]["bytes_read"] == 0
    assert probe["lookup_hits"] == 35_000
    assert len(probe["lookup_samples_ns"]) == 7
    assert len(probe["residual_samples_ns"]) == 7
    fixture_files = sorted(args.probe_fixture.glob("data/r*.json"))
    assert len(fixture_files) == 16_384
    probe_digest = hashlib.sha256()
    probe_metadata = hashlib.sha256()
    for path in fixture_files:
        stat = path.stat()
        probe_digest.update(path.name.encode() + b"\0" + bytes.fromhex(sha256(path)))
        probe_metadata.update(f"{path.name}:{stat.st_size}:{stat.st_mtime_ns}\n".encode())
    validation = {
        "source_commit": manifest["base_commit"],
        "all_required_invocations_passed": True,
        "query_rows": {key: len(value) for key, value in query_rows.items()},
        "canonical_sha256": canonical,
        "probe_binary_sha256": sha256(args.probe_binary),
        "probe_fixture": str(args.probe_fixture.resolve()),
        "probe_fixture_content_sha256": probe_digest.hexdigest(),
        "probe_fixture_metadata_sha256": probe_metadata.hexdigest(),
        "probe_exact_count_zero_source_reads": True,
    }
    (phase / "validation.json").write_text(json.dumps(validation, indent=2) + "\n")
    checksums = []
    for path in sorted(phase.rglob("*")):
        if not path.is_file() or "mutation-fixture" in path.parts or path.name == "checksums.sha256":
            continue
        checksums.append(f"{sha256(path)}  {path.relative_to(phase).as_posix()}")
    (phase / "checksums.sha256").write_text("\n".join(checksums) + "\n")
    print(json.dumps(validation, indent=2))


if __name__ == "__main__":
    main()
