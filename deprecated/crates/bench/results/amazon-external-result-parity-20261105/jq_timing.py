#!/usr/bin/env python3
"""Time jq's complete Amazon ID output from the archive or per-record files."""

import argparse
import gzip
import json
from pathlib import Path
import subprocess
import time


ROOT = Path(__file__).resolve().parents[4]
REPOSITORY = ROOT / "examples/amazon-reviews-2023"
ARCHIVE = REPOSITORY / "archives/Musical_Instruments.jsonl.gz"
FILES = REPOSITORY / "data/reviews"
LIMIT = 1_000_000
FILTER = r'''
    foreach inputs as $record (
        0; . + 1;
        . as $id
        | if ($record.text | type) == "string"
             and (($record.text | contains($needle)) == ($polarity == "positive"))
          then ("0000000" + ($id | tostring))[-7:]
          else empty
          end
    )
'''
NATIVE_FILTER = FILTER.replace("foreach inputs", "foreach limit(1000000; inputs)")


def feed(process: subprocess.Popen, source: str) -> None:
    assert process.stdin is not None
    if source == "archive":
        with gzip.open(ARCHIVE, "rb") as archive:
            for number in range(1, LIMIT + 1):
                line = archive.readline()
                if not line:
                    raise ValueError(f"archive ended before review {number}")
                process.stdin.write(line)
    else:
        for number in range(1, LIMIT + 1):
            process.stdin.write((FILES / f"{number:07d}.json").read_bytes())
    process.stdin.close()


def run(source: str, polarity: str, output: Path) -> float:
    if source == "archive-native":
        return run_native(polarity, output)
    started = time.perf_counter()
    with output.open("wb") as result:
        process = subprocess.Popen(
            ["jq", "-nr", "--arg", "needle", "headphones", "--arg", "polarity", polarity, FILTER],
            stdin=subprocess.PIPE,
            stdout=result,
            stderr=subprocess.PIPE,
        )
        feed(process, source)
        assert process.stderr is not None
        error = process.stderr.read()
        if process.wait() != 0:
            raise RuntimeError(f"jq failed: {error.decode(errors='replace')}")
    return (time.perf_counter() - started) * 1_000.0


def run_native(polarity: str, output: Path) -> float:
    started = time.perf_counter()
    with output.open("wb") as result:
        decompressor = subprocess.Popen(
            ["gzip", "-cd", str(ARCHIVE)],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        assert decompressor.stdout is not None
        parser = subprocess.Popen(
            ["jq", "-nr", "--arg", "needle", "headphones", "--arg", "polarity", polarity, NATIVE_FILTER],
            stdin=decompressor.stdout,
            stdout=result,
            stderr=subprocess.PIPE,
        )
        decompressor.stdout.close()
        _, error = parser.communicate()
        decompressor.wait()
        if parser.returncode:
            raise RuntimeError(f"jq failed: {error.decode(errors='replace')}")
    return (time.perf_counter() - started) * 1_000.0


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", choices=("archive", "archive-native", "files"))
    parser.add_argument("polarity", choices=("positive", "negative"))
    parser.add_argument("output", type=Path)
    parser.add_argument("phase", choices=("first", "warm"))
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    warmup_ms = None
    if args.phase == "warm":
        warmup_ms = run(args.source, args.polarity, Path("/dev/null"))
    query_ms = run(args.source, args.polarity, args.output)
    print(
        json.dumps(
            {
                "engine": "jq",
                "source": args.source,
                "polarity": args.polarity,
                "phase": args.phase,
                "warmup_ms": warmup_ms,
                "query_ms": query_ms,
            }
        )
    )


if __name__ == "__main__":
    main()
