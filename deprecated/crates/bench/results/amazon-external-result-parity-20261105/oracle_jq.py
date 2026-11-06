#!/usr/bin/env python3
"""Use jq to classify exactly the first million archived Amazon review records."""

import gzip
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[4]
OUTPUT = ROOT / "target/benchmarks/amazon-external-result-parity-20261105"
ARCHIVE = ROOT / "examples/amazon-reviews-2023/archives/Musical_Instruments.jsonl.gz"
NEEDLE = "headphones"
LIMIT = 1_000_000
FILTER = r'''
    index("\t") as $separator
    | .[:$separator] as $id
    | .[$separator + 1:] | fromjson
    | if (.text | type) != "string" then "X\t" + $id
      elif (.text | contains($needle)) then "P\t" + $id
      else "N\t" + $id end
'''


def main() -> None:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    classes = OUTPUT / "jq-classification.tsv"
    with classes.open("wb") as result, (OUTPUT / "jq.stderr.log").open("wb") as errors:
        process = subprocess.Popen(
            ["jq", "-Rr", "--arg", "needle", NEEDLE, FILTER],
            stdin=subprocess.PIPE,
            stdout=result,
            stderr=errors,
        )
        assert process.stdin is not None
        with gzip.open(ARCHIVE, "rb") as source:
            for number in range(1, LIMIT + 1):
                line = source.readline()
                if not line:
                    raise ValueError(f"archive ended before review {number}")
                process.stdin.write(f"{number}\t".encode("ascii") + line)
        process.stdin.close()
        if process.wait() != 0:
            raise RuntimeError("jq failed; see jq.stderr.log")

    counts = {"P": 0, "N": 0, "X": 0}
    with (OUTPUT / "jq-positive.ids").open("w") as positive, (
        OUTPUT / "jq-negative.ids"
    ).open("w") as negative, (OUTPUT / "jq-nonstring.ids").open("w") as nonstring:
        sinks = {"P": positive, "N": negative, "X": nonstring}
        with classes.open() as rows:
            expected = 0
            for expected, row in enumerate(rows, start=1):
                label, source_id = row.rstrip("\n").split("\t")
                if int(source_id) != expected:
                    raise ValueError(f"jq source ID mismatch at row {expected}: {row!r}")
                if label not in sinks:
                    raise ValueError(f"jq classification {label!r} is unknown")
                counts[label] += 1
                sinks[label].write(f"{expected:07d}\n")
            if expected != LIMIT:
                raise ValueError(f"jq emitted {expected} rows, expected {LIMIT}")
    print(f"jq classified {LIMIT:,} reviews: {counts}", flush=True)


if __name__ == "__main__":
    main()
