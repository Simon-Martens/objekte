#!/usr/bin/env python3
"""Compare full Musenalm queries before and after binding-cache removal."""

import argparse
import csv
import gzip
import hashlib
import json
import math
import statistics
import subprocess
import time
from pathlib import Path


def digest(path):
    state = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            state.update(chunk)
    return state.hexdigest()


def run(label, mode, program, root, output, repetitions, prime=False):
    result = output / f"{label}-{mode}"
    common = ["--root", str(root), "--extended", "--incremental",
              "--profiles", "ascii-off,unicode-off", "--verified-cache-bytes", "0",
              "--query-workers", "32", "--repetitions", str(repetitions),
              "--result-dir", str(result)]
    if label == "baseline":
        common += ["--prepared-cache-bytes", str(16 * 1024 * 1024)]
    if prime:
        common += ["--cases", "Q001"]
    if mode == "memory":
        common += ["--storage", "memory"]
    else:
        common += ["--storage", "custom", str(output / f"{label}-store")]
    command = ["/usr/bin/time", "-f", "%M", "-o", str(output / f"{label}-{mode}.rss-kb"),
               str(program), *common]
    started = time.monotonic()
    process = subprocess.run(command, capture_output=True, text=True)
    (output / f"{label}-{mode}.log").write_text(process.stdout + "\nSTDERR\n" + process.stderr)
    if process.returncode:
        raise RuntimeError(f"{label} {mode} failed: {process.stderr[-2000:]}")
    if not prime:
        with (result / "measurements.jsonl").open("rb") as raw, gzip.open(
            output / f"{label}-{mode}.jsonl.gz", "wb"
        ) as compressed:
            for chunk in iter(lambda: raw.read(1024 * 1024), b""):
                compressed.write(chunk)
    return {"command": command, "wall_seconds": time.monotonic() - started,
            "peak_rss_kb": int((output / f"{label}-{mode}.rss-kb").read_text()),
            "index_results": json.loads((result / "index-results.json").read_text())}


def query_rows(path):
    with path.open(newline="") as source:
        rows = list(csv.DictReader(source))
    return {(row["case_id"], row["profile"]): row for row in rows}


def main():
    parser = argparse.ArgumentParser()
    for name in ("baseline-program", "candidate-program", "root", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--repetitions", type=int, default=5)
    args = parser.parse_args()
    if args.repetitions < 1 or args.output.exists():
        parser.error("positive repetitions and absent output are required")
    output = args.output.resolve()
    output.mkdir(parents=True)
    programs = {"baseline": args.baseline_program.resolve(),
                "candidate": args.candidate_program.resolve()}
    root = args.root.resolve()
    runs = {}
    for label, program in programs.items():
        (output / f"{label}-store").mkdir()
        runs[f"{label}-prime"] = run(label, "prime", program, root, output, 1, prime=True)
        print(label, "primed", flush=True)
    ratios = {}
    for mode in ("memory", "ondemand"):
        for label, program in programs.items():
            runs[f"{label}-{mode}"] = run(label, mode, program, root, output,
                                           args.repetitions)
            print(label, mode, "done", flush=True)
        before = query_rows(output / f"baseline-{mode}" / "query-results.csv")
        after = query_rows(output / f"candidate-{mode}" / "query-results.csv")
        if before.keys() != after.keys() or len(before) != 440:
            raise RuntimeError(f"case set changed in {mode}")
        if json.loads((output / f"baseline-{mode}" / "result-digests.json").read_text()) != json.loads(
            (output / f"candidate-{mode}" / "result-digests.json").read_text()
        ):
            raise RuntimeError(f"result digests changed in {mode}")
        if mode == "ondemand":
            for label in programs:
                rows = runs[f"{label}-{mode}"]["index_results"]
                if any(row["source_reads"] or row["build_parses"] for row in rows):
                    raise RuntimeError(f"{label} reopened from source")
        ratios[mode] = {}
        for field in ("median_ns", "p95_ns"):
            values = [int(after[key][field]) / max(int(before[key][field]), 1)
                      for key in before]
            ratios[mode][field] = math.exp(statistics.mean(map(math.log, values)))
        ratios[mode]["pairs"] = len(before)
    summary = {"baseline_revision": "8747efc47e (ordinary release behavior at 0c63651458)",
               "candidate_revision": "1e22afd1",
               "program_sha256": {label: digest(program) for label, program in programs.items()},
               "root": str(root), "repetitions": args.repetitions,
               "ratios": ratios, "runs": runs}
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(ratios, indent=2))


if __name__ == "__main__":
    main()
