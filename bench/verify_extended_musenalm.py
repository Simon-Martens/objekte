#!/usr/bin/env python3
"""Independent raw-JSON membership oracle for selected Musenalm relations."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import subprocess
from pathlib import Path


CASES = ("Q157", "Q166", "Q168", "Q169", "Q170", "Q171", "Q172", "Q179", "Q180")


def records(root: Path, collection: str):
    for path in sorted((root / "data" / collection).glob("*.json")):
        yield path.stem, json.loads(path.read_text())


def oracle(root: Path) -> dict[str, list[list[str]]]:
    entries = dict(records(root, "entries"))
    agents = dict(records(root, "agents"))
    places = dict(records(root, "places"))
    expected = {case: set() for case in CASES}
    for identifier, entry in entries.items():
        place_ids = entry.get("places") or []
        first_place = places.get(place_ids[0], {}) if place_ids else {}
        if "Leipzig" in first_place.get("name", ""):
            expected["Q179"].add(("entries", identifier))
        agent_rows = entry.get("agents") or []
        first_agent = agents.get(agent_rows[0].get("agent"), {}) if agent_rows else {}
        if identifier == "00fx8ln651na0e3" and "Krause" in first_agent.get("name", ""):
            expected["Q171"].add(("entries", identifier))
    for identifier, content in records(root, "contents"):
        target_id = content.get("entry")
        target = entries.get(target_id, {})
        if target_id == "jv829b9kekdb5d3":
            expected["Q157"].add(("contents", identifier))
            if "Lieder" in content.get("preferred_title", ""):
                expected["Q170"].add(("entries", target_id))
        if "Taschenbuch" in target.get("preferred_title", ""):
            expected["Q166"].add(("contents", identifier))
        if target_id == "02gua1gg7jny30q":
            place_ids = target.get("places") or []
            first_place = places.get(place_ids[0], {}) if place_ids else {}
            if "Leipzig" in first_place.get("name", ""):
                expected["Q169"].add(("contents", identifier))
        if target_id == "02i35e3nbm7enwp" and any(
            "Roller" in agents.get(row.get("agents"), {}).get("name", "")
            for row in content.get("contributors") or []
        ):
            expected["Q172"].add(("contents", identifier))
    for identifier, series in records(root, "series"):
        entry_ids = series.get("entries") or []
        first = entries.get(entry_ids[0], {}) if entry_ids else {}
        if "Taschenbuch" in first.get("preferred_title", ""):
            expected["Q168"].add(("series", identifier))
        related = series.get("related_entries") or []
        first_related = entries.get(related[0].get("entry"), {}) if related else {}
        if "Taschenbuch" in first_related.get("preferred_title", ""):
            expected["Q180"].add(("series", identifier))
    return {case: [list(row) for row in sorted(found)] for case, found in expected.items()}


def actual(root: Path, alphabet: str, executable: Path) -> dict[str, list[list[str]]]:
    output = subprocess.run(
        [str(executable), "--extended", "--root", str(root), "--alphabet", alphabet,
         "--cases", ",".join(CASES)],
        check=True, text=True, capture_output=True,
    ).stdout
    found = {}
    for line in output.splitlines():
        row = json.loads(line)
        found[row["id"]] = row["ids"]
        if row["count"] != len(row["ids"]):
            raise ValueError(f"{row['id']}: incomplete membership")
    if set(found) != set(CASES):
        raise ValueError("core did not return every selected case")
    return found


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--result-dir", type=Path, required=True)
    parser.add_argument("--executable", type=Path, default=Path("target/release/examples/dump_musenalm_ids"))
    args = parser.parse_args()
    root = args.root.resolve()
    directory = args.result_dir.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    expected = oracle(root)
    results = {"oracle": expected}
    failures = []
    for alphabet in ("ascii", "unicode"):
        results[alphabet] = actual(root, alphabet, args.executable.resolve())
        for case in CASES:
            if results[alphabet][case] != expected[case]:
                failures.append((alphabet, case))
    for label, membership in results.items():
        payload = json.dumps(membership, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
        (directory / f"{label}.json.gz").write_bytes(gzip.compress(payload, mtime=0))
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    digests = {
        label: {
            case: hashlib.sha256(json.dumps(ids, ensure_ascii=False).encode()).hexdigest()
            for case, ids in membership.items()
        }
        for label, membership in results.items()
    }
    (directory / "run-manifest.json").write_text(json.dumps({
        "revision": revision, "root": str(root), "cases": CASES, "comparisons": len(CASES) * 2,
        "failures": failures, "membership_sha256": digests,
    }, indent=2) + "\n")
    lines = ["# Independent Musenalm relation membership oracle", "",
             f"Revision: `{revision}`. Complete ID sets: {len(CASES) * 2 - len(failures)}/{len(CASES) * 2} passed.",
             "The oracle reads original JSON and resolves links by filename ID without using core binding, indexes or comparison helpers.",
             "", "| Case | Expected matches | ASCII | Unicode |", "| --- | ---: | --- | --- |"]
    for case in CASES:
        lines.append(f"| {case} | {len(expected[case])} | {'fail' if ('ascii', case) in failures else 'ok'} | {'fail' if ('unicode', case) in failures else 'ok'} |")
    (directory / "REPORT.md").write_text("\n".join(lines) + "\n")
    if failures:
        raise SystemExit(f"membership mismatches: {failures}")
    print(f"independent relation membership: {len(CASES) * 2}/{len(CASES) * 2} passed")


if __name__ == "__main__":
    main()
