#!/usr/bin/env python3
"""Repeat core-allocation cases with fixed operations and record raw samples."""

import argparse
import json
from pathlib import Path
import subprocess


SYNTHETIC = (
    "cold-build", "view-indexed", "view-fallback", "view-fallback-count",
    "view-direct", "json-sorted", "text-sorted", "json-updates",
    "text-updates", "rebuild", "snapshot-save", "snapshot-restore",
    "json-stream-repeat-off", "json-stream-repeat-on",
    "text-stream-repeat-off", "text-stream-repeat-on",
    "source-lifecycle",
)
MUSENALM = ("json-sorted", "text-sorted", "rebuild", "snapshot-save", "snapshot-restore")
STREAM = ("stream-cache-fixture-off", "stream-cache-fixture-on")


def clear_synthetic_cache(root: Path) -> None:
    if not root.name.startswith("objekte-alloc-fixture") or root.parent != Path("/tmp"):
        raise ValueError("cache clearing is limited to the generated /tmp fixture")
    cache = root / ".objekte" / ".cache"
    if cache.is_dir():
        for path in cache.iterdir():
            if path.is_file():
                path.unlink()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--phase", required=True)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--fixture", type=Path, default=Path("/tmp/objekte-alloc-fixture-v2"))
    parser.add_argument("--musenalm", type=Path, default=Path("examples/musenalm"))
    parser.add_argument("--bin-dir", type=Path, default=Path("target/release"))
    parser.add_argument("--evidence-dir", type=Path)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    binary = args.bin_dir / "core-allocation"
    if args.evidence_dir is not None:
        args.evidence_dir.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", encoding="utf-8") as output:
        for corpus, cases, root in (
            ("synthetic-20k", SYNTHETIC, args.fixture),
            ("musenalm-contents", MUSENALM, args.musenalm),
            ("stream-cache-fixture-2k", STREAM, args.fixture),
        ):
            for case in cases:
                for round_number in range(1, args.runs + 1):
                    if corpus == "synthetic-20k" and case == "cold-build":
                        clear_synthetic_cache(root)
                    command = [str(binary), "--case", case, "--repo", str(root)]
                    if corpus == "musenalm-contents":
                        command += [
                            "--collection", "contents", "--filter", "year >= 1700",
                            "--sort", "annotation ASC", "--pattern", "und",
                        ]
                    completed = subprocess.run(command, capture_output=True, text=True)
                    if args.evidence_dir is not None:
                        label = f"{corpus}-{case}-round{round_number}"
                        (args.evidence_dir / f"{label}.stderr.log").write_text(completed.stderr)
                        (args.evidence_dir / f"{label}.command.json").write_text(json.dumps({
                            "command": command, "exit_status": completed.returncode,
                        }, indent=2) + "\n")
                    completed.check_returncode()
                    sample = json.loads(completed.stdout)
                    sample.update({"phase": args.phase, "corpus": corpus, "round": round_number})
                    output.write(json.dumps(sample, ensure_ascii=False) + "\n")
                    output.flush()
                    print(f"{corpus} {case} {round_number}/{args.runs}", flush=True)


if __name__ == "__main__":
    main()
