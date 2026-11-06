#!/usr/bin/env python3
"""Serialize complete-result jq and Objekte timing samples with output verification."""

import argparse
import hashlib
import json
from pathlib import Path
import random
import subprocess
import time


ROOT = Path(__file__).resolve().parents[4]
HERE = Path(__file__).resolve().parent
RAW = ROOT / "target/benchmarks/amazon-jq-timings-20261105"
RESULTS = HERE / "timings.ndjson"
RUST = ROOT / "target/release/examples/amazon_jq_timing"
JQ = HERE / "jq_timing.py"
REPOSITORY = ROOT / "examples/amazon-reviews-2023"
EXPECTED = {
    ("jq", "positive"): (13_121, "00f0a33918e4e812f42910f438e8f1af6e046e4bc0cc8d79c2001c1356af3004"),
    ("jq", "negative"): (986_879, "c12a3910ff954489948fd487a0602eaa09b4cc9fff2ca1a454f4dd97cbc991cd"),
    ("objekte", "positive"): (13_121, "10ec0df6acfa6d0a3880b54eea301ab65af018952f8299d9495b2240378d6c95"),
    ("objekte", "negative"): (986_879, "3849f59384446ae966f42d7bd005ae3a95bc1f390dda103b2663ff78860f93f3"),
}


def verify(path: Path, engine: str, polarity: str) -> tuple[int, str]:
    digest = hashlib.sha256()
    count = 0
    with path.open("rb") as result:
        for line in result:
            digest.update(line)
            count += 1
    actual = (count, digest.hexdigest())
    expected = EXPECTED[(engine, polarity)]
    if actual != expected:
        raise AssertionError(f"{path}: {actual} != {expected}")
    return actual


def sample(round_number: int, mode: str, polarity: str, phase: str) -> dict:
    engine = "objekte" if mode.startswith("objekte") else "jq"
    name = f"round{round_number}-{mode}-{polarity}-{phase}"
    output = RAW / f"{name}.ids"
    if engine == "objekte":
        backend = mode.removeprefix("objekte-")
        operator = "~" if polarity == "positive" else "!~"
        command = [
            "taskset", "-c", "0-15", "env", "RAYON_NUM_THREADS=16",
            str(RUST), str(REPOSITORY), backend,
            f"text {operator} 'headphones'", str(output), phase,
        ]
    else:
        source = mode.removeprefix("jq-")
        command = [
            "taskset", "-c", "0-15", "python3", str(JQ),
            source, polarity, str(output), phase,
        ]
    started = time.perf_counter()
    process = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, check=False)
    wall_ms = (time.perf_counter() - started) * 1_000.0
    if process.returncode:
        raise RuntimeError(f"{name} exited {process.returncode}: {process.stderr}")
    if process.stderr:
        raise RuntimeError(f"{name} wrote stderr: {process.stderr}")
    details = json.loads(process.stdout)
    count, digest = verify(output, engine, polarity)
    output.unlink()
    return {
        "round": round_number,
        "mode": mode,
        "polarity": polarity,
        "phase": phase,
        "command": command,
        "wall_ms": wall_ms,
        "result_count": count,
        "result_sha256": digest,
        **details,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--modes",
        default="objekte-bigram,objekte-direct,jq-archive,jq-files",
    )
    parser.add_argument("--append", action="store_true")
    args = parser.parse_args()
    RAW.mkdir(parents=True, exist_ok=True)
    rng = random.Random(20261105)
    jobs = [
        (mode, polarity, phase)
        for mode in args.modes.split(",")
        for polarity in ("positive", "negative")
        for phase in ("first", "warm")
    ]
    with RESULTS.open("a" if args.append else "w") as results:
        for round_number in range(1, 4):
            rng.shuffle(jobs)
            for mode, polarity, phase in jobs:
                row = sample(round_number, mode, polarity, phase)
                results.write(json.dumps(row, separators=(",", ":")) + "\n")
                results.flush()
                print(
                    f"round {round_number}: {mode} {polarity} {phase}: "
                    f"{row['query_ms']:.1f} ms, {row['result_count']:,} IDs",
                    flush=True,
                )


if __name__ == "__main__":
    main()
