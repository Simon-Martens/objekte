#!/usr/bin/env python3
"""Run a verified release comparison of Objekte and Tantivy on Musenalm."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import statistics
import subprocess
from collections import defaultdict
from pathlib import Path


REPOSITORY = Path(__file__).resolve().parent.parent
BINARY = REPOSITORY / "target/release/objekte-tantivy-benchmark"
QUICK = ("Q001", "Q030", "Q035", "Q044", "Q054", "Q081", "Q116", "Q136", "Q144")


def command(*args: str) -> str:
    result = subprocess.run(args, cwd=REPOSITORY, text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError(f"{' '.join(args)} failed with exit {result.returncode}\n{result.stderr}")
    return result.stdout.strip()


def file_hash(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def corpus_fingerprint(root: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    total = 0
    for schema in sorted((root / ".objekte/collections").glob("*.schema.json")):
        declaration = json.loads(schema.read_text())["x-objekte"]
        for path in sorted((root / declaration["path"]).glob("*.json")):
            info = path.stat()
            digest.update(str(path.relative_to(root)).encode())
            digest.update(info.st_size.to_bytes(8, "little"))
            digest.update(info.st_mtime_ns.to_bytes(16, "little", signed=True))
            total += 1
    return digest.hexdigest(), total


def prime_sources(root: Path) -> None:
    for schema in sorted((root / ".objekte/collections").glob("*.schema.json")):
        declaration = json.loads(schema.read_text())["x-objekte"]
        for path in sorted((root / declaration["path"]).glob("*.json")):
            path.read_bytes()


def family(row: dict) -> str:
    number = int(row["source_id"][1:])
    if number <= 30:
        return "literal FTS"
    if number <= 61:
        return "scalar and numeric filters"
    if number <= 79:
        return "string filters"
    if number <= 105:
        return "array and nested filters"
    if number <= 125:
        return "structure and metadata"
    if number <= 140:
        return "sorted pages"
    if number <= 150:
        return "mixed requests"
    return "extended requests"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("quick", "full"))
    parser.add_argument("--root", type=Path, default=REPOSITORY / "examples/musenalm")
    parser.add_argument("--result-dir", type=Path, required=True)
    parser.add_argument("--cases", help="comma-separated source IDs")
    parser.add_argument("--query-workers", type=int, default=1)
    parser.add_argument("--build-workers", type=int, default=1)
    parser.add_argument("--repetitions", type=int)
    args = parser.parse_args()
    if args.query_workers < 1 or args.build_workers < 1:
        parser.error("worker counts must be positive")
    repetitions = args.repetitions or (1 if args.mode == "quick" else 7)
    if repetitions < 1:
        parser.error("repetitions must be positive")
    root = args.root.resolve(strict=True)
    result_dir = args.result_dir.resolve()
    if result_dir.exists():
        parser.error(f"result directory already exists: {result_dir}")
    result_dir.mkdir(parents=True)
    command("cargo", "build", "--release", "--locked", "-p", "objekte-tantivy-benchmark")
    command(str(BINARY), "inventory", str(root), str(result_dir / "compatibility-inventory.json"))
    rows = json.loads((result_dir / "compatibility-inventory.json").read_text())
    by_id = {row["source_id"]: row for row in rows}
    selected = args.cases.split(",") if args.cases else list(QUICK if args.mode == "quick" else by_id)
    if len(selected) != len(set(selected)) or any(case not in by_id for case in selected):
        parser.error("case selection has duplicate or unknown IDs")

    eligible = [case for case in selected if by_id[case]["predicate_status"] == "pending_verification"]
    verification = []
    modes = [mode for mode in ("historical", "extended") if any(by_id[case]["schema_mode"] == mode for case in eligible)]
    for mode in modes:
        mode_cases = [case for case in eligible if by_id[case]["schema_mode"] == mode]
        destination = result_dir / f"verification-{mode}.json"
        command(str(BINARY), "verify-counts", str(root), str(destination), ",".join(mode_cases))
        verification.extend(json.loads(destination.read_text()))
    (result_dir / "verification.json").write_text(json.dumps(verification, indent=2) + "\n")
    verified = {row["id"]: row for row in verification}
    if set(verified) != set(eligible):
        raise RuntimeError("verification omitted an eligible source case")
    for case in eligible:
        row = by_id[case]
        row["predicate_status"] = "compatible_verified"
        if row["page_status"] == "pending_verification":
            row["page_status"] = "compatible_verified"

    aliases = {}
    representatives = {}
    for case in eligible:
        row = by_id[case]
        for contract, field, status in (
            ("predicate_count", "effective_count", "predicate_status"),
            ("ordered_locator_page", "effective_page", "page_status"),
        ):
            if row[status] != "compatible_verified":
                continue
            key = (row["schema_mode"], contract, json.dumps(row[field], ensure_ascii=False, sort_keys=True, separators=(",", ":")))
            representative = representatives.setdefault(key, case)
            aliases[(case, contract)] = representative
            row[contract + "_measurement_id"] = representative
    selectors = []
    for case in selected:
        for contract, suffix in (("predicate_count", "count"), ("ordered_locator_page", "page")):
            if aliases.get((case, contract)) == case:
                selectors.append(f"{case}@{suffix}")
    (result_dir / "compatibility-inventory.json").write_text(json.dumps(rows, ensure_ascii=False, indent=2) + "\n")

    fingerprint, documents = corpus_fingerprint(root)
    if documents != 148486:
        raise RuntimeError(f"unexpected source universe: {documents}")
    manifest = {
        "revision": command("git", "rev-parse", "HEAD"),
        "root": str(root),
        "corpus_fingerprint": fingerprint,
        "documents": documents,
        "schema_fingerprints": {path.name: file_hash(path) for path in sorted((root / ".objekte/collections").glob("*.schema.json"))},
        "historical_workload_sha256": file_hash(REPOSITORY / "crates/core/benches/musenalm_queries.json"),
        "extended_workload_sha256": file_hash(REPOSITORY / "crates/core/benches/musenalm_queries_extended.json"),
        "lockfile_sha256": file_hash(REPOSITORY / "Cargo.lock"),
        "tantivy_version": "0.27.0",
        "rustc": command("rustc", "--version"),
        "host": platform.platform(),
        "cpu_affinity": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
        "release_configuration": "Cargo release profile, cargo build --release --locked",
        "mode": args.mode,
        "selected_cases": selected,
        "repetitions": repetitions,
        "engine_order": [f"{engine}-{mode}" for mode in modes for engine in ("objekte", "tantivy")],
        "permutation": "case list rotated by repetition index, seed 0",
        "query_workers": args.query_workers,
        "build_workers": args.build_workers,
        "verified_result_cache_bytes": 0,
        "objekte_storage": "in-memory",
        "tantivy_storage": "RAM directory",
        "source_cache": "each measured build preceded by a complete read of all declared source files",
        "command": " ".join(["python3", "bench/run_musenalm_tantivy.py", args.mode, "--root", str(root), "--result-dir", str(result_dir)]),
    }
    (result_dir / "run-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    runs = {engine: {"builds": {}, "samples": []} for engine in ("objekte", "tantivy")}
    if selectors:
        for mode in modes:
            mode_selectors = [selector for selector in selectors if by_id[selector.split("@")[0]]["schema_mode"] == mode]
            for engine in ("objekte", "tantivy"):
                destination = result_dir / f"{engine}-{mode}-samples.json"
                prime_sources(root)
                command(str(BINARY), f"measure-{engine}", str(root), str(destination), ",".join(mode_selectors), str(repetitions), str(args.build_workers), str(args.query_workers))
                observation = json.loads(destination.read_text())
                runs[engine]["builds"][mode] = observation["build"]
                runs[engine]["samples"].extend(observation["samples"])
        for engine, run in runs.items():
            (result_dir / f"{engine}-samples.json").write_text(json.dumps(run, indent=2) + "\n")

    grouped = defaultdict(lambda: defaultdict(list))
    for engine, run in runs.items():
        for sample in run["samples"]:
            key = (sample["id"], sample["contract"])
            grouped[key][engine].append(sample["elapsed_ns"])
            if sample["count"] != verified[sample["id"]]["count"]:
                raise RuntimeError(f"{engine} sample has wrong count for {key}")
            if sample["contract"] == "predicate_count" and sample["page_len"] != 0:
                raise RuntimeError(f"{engine} count returned hits for {key}")
            if sample["contract"] == "ordered_locator_page":
                source = by_id[sample["id"]]["effective_page"]
                expected_length = min(source["limit"], max(0, sample["count"] - source["offset"]))
                if sample["page_len"] != expected_length:
                    raise RuntimeError(f"{engine} page length differs for {key}")
    summary = []
    for (case, contract), samples in grouped.items():
        if any(len(samples[engine]) != repetitions for engine in ("objekte", "tantivy")):
            raise RuntimeError(f"missing samples for {case}/{contract}")
        native = statistics.median(samples["objekte"])
        other = statistics.median(samples["tantivy"])
        row = by_id[case]
        summary.append({
            "id": case,
            "aliases": [source for source in selected if aliases.get((source, contract)) == case],
            "contract": contract,
            "family": family(row),
            "collection": row["original"]["collection"],
            "representation": row["representation"],
            "count": verified[case]["count"],
            "offset": row["effective_page"]["offset"] if contract == "ordered_locator_page" else 0,
            "page_length": row["effective_page"]["limit"] if contract == "ordered_locator_page" else 0,
            "samples": repetitions,
            "objekte_median_ns": native,
            "tantivy_median_ns": other,
            "tantivy_over_objekte": other / native if native else None,
            "objekte_min_max_ns": [min(samples["objekte"]), max(samples["objekte"])],
            "tantivy_min_max_ns": [min(samples["tantivy"]), max(samples["tantivy"])],
        })
    summary.sort(key=lambda row: (row["family"], row["contract"], row["id"]))
    (result_dir / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n")
    report = [
        "# Musenalm release comparison", "",
        f"Revision `{manifest['revision']}`. Verified {len(eligible)} of {len(selected)} selected predicates and {sum(row['page_status'] == 'compatible_verified' for row in rows)} source pages.", "",
        "Page timings cover locator tuples with evidence and payloads projected away. Ratios above 1 favor Objekte. Build times are one observation per engine and schema mode.", "",
    ]
    for engine in ("objekte", "tantivy"):
        for mode, build in runs[engine]["builds"].items():
            size = f"{build['index_bytes'] / 1048576:.1f} MiB of managed RAM-directory segment files" if build["index_bytes"] is not None else "index byte size unavailable from the in-memory core"
            report.append(f"{engine.title()} {mode} built {build['documents']:,} visible records in {build['elapsed_ns'] / 1e9:.3f} s. Peak RSS was {build['peak_rss_kb'] / 1024:.1f} MiB and resident RSS at query readiness was {build['resident_ready_kb'] / 1024:.1f} MiB. It has {build['segments']} searchable segments and {size}.")
    report.extend(["", "## Per-request samples", ""])
    current_group = None
    for row in summary:
        group = (row["family"], row["contract"])
        if group != current_group:
            report.extend([f"### {group[0]} — {group[1]}", "", "| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |", "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |"])
            current_group = group
        source = by_id[row["id"]]["original"]
        predicate = (source.get("fts") or source.get("filter") or "")[:45].replace("|", "\\|")
        native_range = row["objekte_min_max_ns"]
        other_range = row["tantivy_min_max_ns"]
        report.append(f"| {', '.join(row['aliases'])} {predicate} | {row['collection']} | {row['representation']} | {row['count']:,} | {row['offset']} / {row['page_length']} | {row['samples']} | {row['objekte_median_ns'] / 1000:.1f} [{native_range[0] / 1000:.1f}–{native_range[1] / 1000:.1f}] µs | {row['tantivy_median_ns'] / 1000:.1f} [{other_range[0] / 1000:.1f}–{other_range[1] / 1000:.1f}] µs | {row['tantivy_over_objekte']:.3g} |")
    report.extend(["", "## Largest absolute median differences", ""])
    for row in sorted(summary, key=lambda item: abs(item["tantivy_median_ns"] - item["objekte_median_ns"]), reverse=True)[:10]:
        report.append(f"- {row['id']} {row['contract']} matched {row['count']:,} records. Objekte {row['objekte_median_ns'] / 1e6:.3f} ms, Tantivy {row['tantivy_median_ns'] / 1e6:.3f} ms.")
    exclusions = [by_id[case] for case in selected if by_id[case]["predicate_status"] == "unsupported"]
    report.extend(["", f"## Unsupported predicates ({len(exclusions)})", ""])
    for row in exclusions:
        report.append(f"- {row['source_id']} — {row['predicate_reason']}")
    page_exclusions = [by_id[case] for case in selected if by_id[case]["predicate_status"] == "compatible_verified" and by_id[case]["page_status"] == "unsupported"]
    report.extend(["", f"## Verified counts with unsupported pages ({len(page_exclusions)})", ""])
    for row in page_exclusions:
        report.append(f"- {row['source_id']} — {row['page_reason']}")
    report.extend(["", "Raw samples, complete inventory, verification counts and run manifest are in this result directory.", ""])
    (result_dir / "REPORT.md").write_text("\n".join(report))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
