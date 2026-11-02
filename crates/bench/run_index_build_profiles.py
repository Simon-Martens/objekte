#!/usr/bin/env python3
"""Run isolated cold index builds across the example repositories."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time


ROOT = Path(__file__).resolve().parents[2]
BENCH = ROOT / "target/release/index-build-profile"
DEFAULT_OUTPUT = ROOT / "crates/bench/results/index-build-profile-2026-11-02.ndjson"

PROFILES = [
    ("lenz-base", "lenz-archiv", "sekundaerliteratur", "ascii", []),
    ("lenz-fts2", "lenz-archiv", "sekundaerliteratur", "ascii", ["Eintrag=plain", "Autor=plain"]),
    ("musenalm-base", "musenalm", "contents", "ascii", []),
    ("musenalm-fts4", "musenalm", "contents", "ascii", [
        "annotation=markdown", "preferred_title=plain", "title_statement=html",
        "contributors[].annotation=xml",
    ]),
    ("leipzig-base", "leipzig-multilingual", "texts", "utf8_bytes", []),
    ("leipzig-fts1", "leipzig-multilingual", "texts", "utf8_bytes", ["text=plain"]),
    ("amazon-base", "amazon-reviews-2023", "reviews", "ascii", []),
    ("amazon-fts2", "amazon-reviews-2023", "reviews", "ascii", ["title=plain", "text=plain"]),
    ("nyc-base", "nyc311", "requests", "ascii", []),
    ("nyc-fts3", "nyc311", "requests", "ascii", [
        "complaint_type=plain", "descriptor=plain", "resolution_description=plain",
    ]),
]


def manifest_hash(repo: Path) -> str | None:
    manifest = repo / "import-manifest.json"
    if manifest.is_file():
        return hashlib.sha256(manifest.read_bytes()).hexdigest()
    return None


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--only", help="comma-separated profile names")
    parser.add_argument("--resume", action="store_true")
    parser.add_argument("--no-build", action="store_true")
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    wanted = set(args.only.split(",")) if args.only else None
    profiles = [profile for profile in PROFILES if wanted is None or profile[0] in wanted]
    if not profiles:
        parser.error("no profiles selected")
    if wanted and wanted - {profile[0] for profile in profiles}:
        parser.error(f"unknown profiles: {', '.join(sorted(wanted - {p[0] for p in profiles}))}")

    if not args.no_build:
        subprocess.run(
            ["cargo", "build", "--release", "-p", "objekte-bench", "--features", "index-timings",
             "--bin", "index-build-profile"],
            cwd=ROOT, check=True,
        )
    completed = set()
    if args.resume and args.output.is_file():
        for line in args.output.read_text().splitlines():
            record = json.loads(line)
            completed.add((record["profile"], record["round"]))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    mode = "a" if args.resume else "w"
    with args.output.open(mode, encoding="utf-8") as output:
        for round_number in range(1, args.runs + 1):
            ordered = profiles if round_number % 2 else list(reversed(profiles))
            for name, repo_name, collection, encoding, fields in ordered:
                if (name, round_number) in completed:
                    continue
                repo = ROOT / "examples" / repo_name
                if not repo.is_dir():
                    raise FileNotFoundError(f"materialize {repo} before benchmarking")
                command = [str(BENCH), "--repo", str(repo), "--collection", collection,
                           "--encoding", encoding]
                for field in fields:
                    command += ["--field", field]
                started = time.monotonic()
                result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
                if result.returncode:
                    raise RuntimeError(f"{name} failed: {result.stderr}")
                record = json.loads(result.stdout)
                record.update({
                    "profile": name,
                    "round": round_number,
                    "source_manifest_sha256": manifest_hash(repo),
                })
                output.write(json.dumps(record, sort_keys=True) + "\n")
                output.flush()
                print(f"{name} round {round_number}/{args.runs}: "
                      f"{record['build_wall_nanos']/1e9:.2f}s, "
                      f"{record['compressed_index_bytes']/1e6:.1f} MB "
                      f"({time.monotonic()-started:.1f}s including snapshot)", flush=True)


if __name__ == "__main__":
    main()
