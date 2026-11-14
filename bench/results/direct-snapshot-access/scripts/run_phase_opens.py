#!/usr/bin/env python3
"""Explain managed-open work with the detailed-profiling release example."""

import argparse
import hashlib
import json
import os
import statistics
import subprocess
from pathlib import Path


def parse_phase(line):
    parts = line.split()
    if not parts or not parts[0].startswith("direct-"):
        return None
    return parts[0], {key: int(value) for key, value in
                      (part.split("=", 1) for part in parts[1:])}


def main():
    parser = argparse.ArgumentParser()
    for name in ("program", "root", "store", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--trials", type=int, default=3)
    args = parser.parse_args()
    if args.trials < 1 or args.output.exists():
        parser.error("positive trial count and absent output are required")
    output = args.output.resolve()
    output.mkdir(parents=True)
    program = args.program.resolve()
    root = args.root.resolve()
    store = args.store.resolve()
    samples = []
    for trial in range(1, args.trials + 1):
        for resident in (False, True):
            for alphabet in ("ascii", "unicode"):
                command = [str(program), "--root", str(root), "--store", str(store),
                           "--alphabet", alphabet]
                if resident:
                    command.append("--resident")
                run = subprocess.run(command, capture_output=True, text=True,
                                     env=dict(os.environ, OBJEKTE_DIRECT_OPEN_PHASES="1"))
                label = f"{trial}-{alphabet}-{'resident' if resident else 'ondemand'}"
                (output / f"{label}.stderr").write_text(run.stderr)
                if run.returncode:
                    raise RuntimeError(f"{label}: {run.stderr[-2000:]}")
                summary = json.loads(run.stdout)
                if summary["source_reads"] or summary["build_parses"]:
                    raise RuntimeError(f"{label}: open reached source files")
                phases = [parsed for line in run.stderr.splitlines()
                          if (parsed := parse_phase(line))]
                by_name = {}
                for name, fields in phases:
                    by_name.setdefault(name, []).append(fields)
                if len(by_name.get("direct-open", [])) != 2 or len(by_name.get("direct-restore", [])) != 1:
                    raise RuntimeError(f"{label}: incomplete open-phase trace")
                segments = by_name.get("direct-segment", [])
                if not segments:
                    raise RuntimeError(f"{label}: no segment-phase trace")
                sample = {"trial": trial, "alphabet": alphabet, "resident": resident,
                          "command": command, "open": summary,
                          "manifest": by_name["direct-open"][0],
                          "generation": by_name["direct-open"][1],
                          "restore": by_name["direct-restore"][0],
                          "segments": segments,
                          "segment_totals": {key: sum(segment[key] for segment in segments)
                                             for key in ("slots", "pre_record_ns",
                                                         "record_validation_ns", "dictionary_install_ns",
                                                         "sort_relation_ns", "final_checks_ns", "total_ns")}}
                samples.append(sample)
                print(label, "done", flush=True)
    medians = {}
    for resident in (False, True):
        for alphabet in ("ascii", "unicode"):
            group = [sample for sample in samples if sample["resident"] == resident
                     and sample["alphabet"] == alphabet]
            key = f"{alphabet}-{'resident' if resident else 'ondemand'}"
            medians[key] = {f"{part}_{field}": statistics.median(row[part][field] for row in group)
                            for part, fields in (("open", ("open_ns", "allocation_calls",
                                                            "gross_allocated_bytes", "metadata_ms",
                                                            "full_available_ms")),
                                                 ("manifest", ("manifest_ns",)),
                                                 ("generation", ("catalog_read_ns", "segment_map_parse_ns",
                                                                 "section_check_ns", "other_ns", "total_ns")),
                                                 ("restore", ("catalog_validation_ns", "owner_ns",
                                                              "segment_install_ns", "catalog_install_ns",
                                                              "live_validation_ns", "publication_ns", "total_ns")),
                                                 ("segment_totals", ("slots", "record_validation_ns",
                                                                     "dictionary_install_ns", "sort_relation_ns",
                                                                     "final_checks_ns", "total_ns")))
                            for field in fields}
    result = {"program_sha256": hashlib.sha256(program.read_bytes()).hexdigest(),
              "root": str(root), "store": str(store), "trials": args.trials,
              "medians": medians, "samples": samples}
    (output / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(medians, indent=2))


if __name__ == "__main__":
    main()
