#!/usr/bin/env python3
"""Repeat both bigram encodings on real and synthetic inputs in fresh processes."""

import argparse
import json
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=5)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    signatures = {}
    for run in range(1, args.runs + 1):
        for source in ("examples/musenalm/data/contents", "synthetic:24000"):
            for encoding in ("ascii", "utf8_bytes"):
                name = f"{'musenalm' if source.startswith('examples/') else 'synthetic'}-{encoding}-{run}"
                command = [str(args.binary), encoding, source]
                completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
                (args.output_dir / f"{name}.json").write_text(completed.stdout)
                (args.output_dir / f"{name}.stderr.log").write_text(completed.stderr)
                (args.output_dir / f"{name}.command.json").write_text(json.dumps({
                    "command": command, "exit_status": completed.returncode,
                }, indent=2) + "\n")
                if completed.returncode:
                    raise RuntimeError(f"{name} exited {completed.returncode}; see stderr log")
                result = json.loads(completed.stdout)
                if result["encoding"] != encoding or result["dataset"] != source:
                    raise ValueError(f"{name}: unexpected encoding or dataset")
                signature = {"files": result["files"],
                             "candidates": {row["term"]: row["candidates"]
                                            for row in result["queries"]}}
                key = (source, encoding)
                if key in signatures and signature != signatures[key]:
                    raise ValueError(f"{name}: candidate counts changed between repetitions")
                signatures[key] = signature
                print(f"validated {name}: {result['files']} files", flush=True)
    (args.output_dir / "signatures.json").write_text(json.dumps(
        {f"{source}/{encoding}": signature for (source, encoding), signature in signatures.items()},
        indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
