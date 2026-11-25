#!/usr/bin/env python3
"""Screen one candidate, then interleave only material candidate outliers.

Individual operations below one millisecond are informational. A wrong answer
or changed work counter is validated independently and always fails.
"""

import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess


QUERY_PHASES = ("historical-ascii", "historical-unicode", "focused-ascii",
                "focused-unicode", "extended-unicode")
MATCH_FIELDS = ("status", "total_matches", "returned_records", "candidates",
                "files_opened", "bytes_read", "record_parses")


def digest(path):
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(block)
    return checksum.hexdigest()


def query_rows(root, phase):
    path = root / phase / "results/query-results.csv"
    rows = list(csv.DictReader(path.open()))
    result = {row["case_id"]: row for row in rows}
    if len(result) != len(rows):
        raise ValueError(f"duplicate case ID in {path}")
    samples = {}
    for line in (root / phase / "results/measurements.jsonl").read_text().splitlines():
        sample = json.loads(line)
        samples.setdefault(sample["case_id"], []).append(sample["repetition"])
    expected = 1 if phase.startswith("historical") else 7
    if set(samples) != set(result) or any(sorted(reps) != list(range(expected))
                                         for reps in samples.values()):
        raise ValueError(f"missing or duplicate sample sequence in {phase}")
    return result


def lifecycle_rows(root):
    rows = []
    for sample in (1, 2, 3):
        path = root / f"lifecycle-small-{sample}/stdout.log"
        entries = list(csv.DictReader(path.open(), delimiter="\t"))
        result = {entry["phase"]: entry for entry in entries}
        if len(result) != len(entries):
            raise ValueError(f"duplicate lifecycle phase in {path}")
        rows.append(result)
    if not all(set(row) == set(rows[0]) for row in rows):
        raise ValueError("lifecycle sample phases differ")
    return {phase: statistics.median(float(row[phase]["ms"]) for row in rows)
            for phase in rows[0]}


def material(before, after, unit):
    """Individual-operation deltas below one millisecond are informational."""
    one_ms = 1_000_000 if unit == "ns" else 1.0
    return after > before * 1.1 and after - before >= one_ms


def screen(before_root, after_root):
    selection = {}
    rows = []
    for phase in QUERY_PHASES:
        before, after = query_rows(before_root, phase), query_rows(after_root, phase)
        if set(before) != set(after):
            raise ValueError(f"{phase}: case IDs differ")
        for case_id in sorted(before):
            for field in MATCH_FIELDS:
                if before[case_id][field] != after[case_id][field]:
                    raise ValueError(f"{phase}/{case_id}: {field} differs")
            old, new = int(before[case_id]["median_ns"]), int(after[case_id]["median_ns"])
            triggered = material(old, new, "ns")
            rows.append({"phase": phase, "case_id": case_id, "before_ns": old,
                         "after_ns": new, "classification": "confirm" if triggered else
                         ("informational" if abs(new - old) < 1_000_000
                          else "pass")})
            if triggered:
                selection.setdefault(phase, set()).add(case_id)
        if phase.startswith("focused"):
            old = sum(int(row["median_ns"]) for row in before.values())
            new = sum(int(row["median_ns"]) for row in after.values())
            if new > old * 1.1:
                selection.setdefault(phase, set()).update(before)
                rows.append({"phase": phase, "case_id": "aggregate", "before_ns": old,
                             "after_ns": new, "classification": "confirm"})
    old_life, new_life = lifecycle_rows(before_root), lifecycle_rows(after_root)
    if set(old_life) != set(new_life):
        raise ValueError("lifecycle phase IDs differ")
    for phase in sorted(old_life):
        old, new = old_life[phase], new_life[phase]
        triggered = material(old, new, "ms")
        rows.append({"phase": "lifecycle-small", "case_id": phase, "before_ms": old,
                     "after_ms": new, "classification": "confirm" if triggered else
                     ("informational" if abs(new - old) < 1.0 else "pass")})
        if triggered:
            selection.setdefault("lifecycle-small", set()).add(phase)
    for alphabet in ("ascii", "unicode"):
        for metric in ("wall_ns", "peak_resident_bytes"):
            old = statistics.median(json.loads((before_root / f"build-{alphabet}-{sample}/stdout.log")
                                               .read_text())[metric] for sample in (1, 2, 3))
            new = statistics.median(json.loads((after_root / f"build-{alphabet}-{sample}/stdout.log")
                                               .read_text())[metric] for sample in (1, 2, 3))
            if new > old * 1.1:
                selection.setdefault(f"build-{alphabet}", set()).add(metric)
                rows.append({"phase": f"build-{alphabet}", "case_id": metric,
                             "before": old, "after": new, "classification": "confirm"})
    old_probe = json.loads((before_root / "direct-probe.json").read_text())
    new_probe = json.loads((after_root / "direct-probe.json").read_text())
    for field in ("exact_count", "residual_count", "lookup_hits", "records",
                  "lookup_operations_per_sample", "exact_counters", "residual_counters"):
        if old_probe[field] != new_probe[field]:
            raise ValueError(f"direct probe changed {field}")
    if new_probe["exact_counters"]["files_opened"] or new_probe["exact_counters"]["bytes_read"]:
        raise ValueError("exact probe performed source I/O")
    for field, case in (("residual_samples_ns", "residual"), ("lookup_samples_ns", "lookup")):
        if len(old_probe[field]) != 7 or len(new_probe[field]) != 7:
            raise ValueError(f"wrong probe sample count: {field}")
        old, new = statistics.median(old_probe[field]), statistics.median(new_probe[field])
        if case == "lookup":
            old /= old_probe["lookup_operations_per_sample"]
            new /= new_probe["lookup_operations_per_sample"]
        triggered = material(old, new, "ns")
        rows.append({"phase": "direct-probe", "case_id": case,
                     "before_ns": old, "after_ns": new,
                     "classification": "confirm" if triggered else "informational"})
        if triggered:
            selection.setdefault("direct-probe", set()).add(case)
    return selection, rows


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline-bin", type=Path, required=True)
    parser.add_argument("--after-bin", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--baseline-worktree", type=Path, required=True)
    parser.add_argument("--after-worktree", type=Path, required=True)
    parser.add_argument("--before-evidence", type=Path, required=True)
    parser.add_argument("--after-evidence", type=Path, required=True)
    parser.add_argument("--baseline-probe-bin", type=Path)
    parser.add_argument("--after-probe-bin", type=Path)
    parser.add_argument("--probe-fixture", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=False)
    before_root, after_root = args.before_evidence.resolve(strict=True), args.after_evidence.resolve(strict=True)
    base, candidate = (json.loads((root / "manifest.json").read_text())
                       for root in (before_root, after_root))
    for field in ("fixture", "fixture_fingerprint", "cargo_lock_sha256",
                  "historical_manifest_sha256", "extended_manifest_sha256",
                  "affinity", "governors", "workers", "query_workers", "rustflags", "allocator"):
        if base[field] != candidate[field]:
            raise ValueError(f"mismatched {field}")
    if any(entry["exit_status"] for manifest in (base, candidate)
           for entry in manifest["invocations"]):
        raise ValueError("matrix has failed invocations")
    selection, screening = screen(before_root, after_root)
    if "direct-probe" in selection and not all((
        args.baseline_probe_bin, args.after_probe_bin, args.probe_fixture,
    )):
        raise ValueError("material direct probe requires probe binaries and fixture")
    fixture = args.fixture.resolve(strict=True)
    if str(fixture) != candidate["fixture"]:
        raise ValueError("fixture path differs from candidate")
    files = sorted(fixture.rglob("*.json"))
    sides = {
        "before": (args.baseline_bin.resolve(strict=True), args.baseline_worktree.resolve(strict=True)),
        "after": (args.after_bin.resolve(strict=True), args.after_worktree.resolve(strict=True)),
    }
    for side, (binary, worktree) in sides.items():
        manifest = base if side == "before" else candidate
        if subprocess.check_output(["git", "-C", str(worktree), "rev-parse", "HEAD"],
                                   text=True).strip() != manifest["base_commit"]:
            raise ValueError(f"{side}: evidence commit does not match worktree")
        for name in ("bench_build_musenalm", "bench_musenalm", "bench_lifecycle_mutations"):
            if digest(binary / name) != manifest["binaries"][name]["sha256"]:
                raise ValueError(f"{side}: {name} differs from measured binary")
    manifest = {
        "fixture": str(fixture), "fixture_fingerprint": candidate["fixture_fingerprint"],
        "pairs": 3, "first_side": ["before", "after", "before"],
        "screening": screening,
        "selected": {phase: sorted(cases) for phase, cases in selection.items()},
        "probe_fixture": str(args.probe_fixture.resolve(strict=True)) if args.probe_fixture else None,
        "probe_binaries": {
            "before": digest(args.baseline_probe_bin.resolve(strict=True)) if args.baseline_probe_bin else None,
            "after": digest(args.after_probe_bin.resolve(strict=True)) if args.after_probe_bin else None,
        },
        "sides": {side: {"sha": (base if side == "before" else candidate)["base_commit"],
                        "build_binary_sha256": digest(binary / "bench_build_musenalm"),
                        "query_binary_sha256": digest(binary / "bench_musenalm"),
                        "lifecycle_binary_sha256": digest(binary / "bench_lifecycle_mutations")}
                  for side, (binary, _) in sides.items()},
        "invocations": [],
    }
    workers = min(32, len(os.sched_getaffinity(0)))

    def invoke(label, side, command, environment=None):
        for path in files:
            with path.open("rb") as stream:
                while stream.read(1024 * 1024):
                    pass
        directory = output / label
        directory.mkdir(parents=True)
        stdout, stderr = directory / "stdout.log", directory / "stderr.log"
        with stdout.open("wb") as out, stderr.open("wb") as err:
            status = subprocess.run(command, cwd=sides[side][1], stdout=out, stderr=err,
                                    env={**os.environ, **(environment or {})}).returncode
        manifest["invocations"].append({
            "label": label, "side": side, "command": list(map(str, command)),
            "exit_status": status, "stdout_sha256": digest(stdout),
            "stderr_sha256": digest(stderr),
        })
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"{label}: exit {status}", flush=True)
        if status:
            raise RuntimeError(f"{label} failed; see {stderr}")

    for phase in selection:
        for pair in range(1, 4):
            order = ("before", "after") if pair % 2 else ("after", "before")
            for side in order:
                binary = sides[side][0]
                label = f"{phase}-pair-{pair}-{side}"
                if phase.startswith("build-"):
                    command = [binary / "bench_build_musenalm", "--root", fixture,
                               "--alphabet", phase.removeprefix("build-"),
                               "--build-workers", str(workers)]
                    environment = None
                elif phase in QUERY_PHASES:
                    alphabet = "ascii" if phase.endswith("ascii") else "unicode"
                    command = [binary / "bench_musenalm", "--root", fixture,
                               "--profiles", f"{alphabet}-off", "--verified-cache-bytes", "0",
                               "--query-workers", "1", "--cases", ",".join(sorted(selection[phase])),
                               "--repetitions", "7", "--result-dir", output / label / "results"]
                    if phase == "extended-unicode":
                        command.insert(1, "--extended")
                    environment = None
                elif phase == "direct-probe":
                    probe_binary = (args.baseline_probe_bin if side == "before"
                                    else args.after_probe_bin).resolve(strict=True)
                    command = [probe_binary, "probe", args.probe_fixture.resolve(strict=True)]
                    environment = None
                else:
                    command = [binary / "bench_lifecycle_mutations", "small",
                               output / label / "mutation-fixture"]
                    environment = {"OBJEKTE_BENCH_MUTATE_COPY": "1"}
                invoke(label, side, command, environment)
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (output / "completed").write_text("complete\n")


if __name__ == "__main__":
    main()
