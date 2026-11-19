#!/usr/bin/env python3

import argparse
import json
import re
from pathlib import Path


TIMESTAMP = re.compile(r"^\d+\s+(\d+\.\d+)\s+(\w+)\(")
QUOTED_PATH = re.compile(r'"(/[^"]+/data/[^"]+)"')
FD_PATH = re.compile(r"\d+<(/[^>]+/data/[^>]+)>")


def timestamp_ns(line):
    match = TIMESTAMP.match(line)
    return None if match is None else round(float(match.group(1)) * 1_000_000_000)


def call_name(line):
    match = TIMESTAMP.match(line)
    return None if match is None else match.group(2)


def result_number(line):
    match = re.search(r"\)\s+=\s+(-?\d+)", line)
    return None if match is None else int(match.group(1))


def relative_path(fixture_root, path):
    return str(Path(path).relative_to(fixture_root))


def marker_intervals(markers_dir):
    intervals = {}
    for path in sorted(markers_dir.glob("*.markers")):
        start, end = path.read_text().split()
        intervals[path.stem] = (int(start), int(end))
    expected = {"exact_page", "combined_old", "combined_number"}
    if set(intervals) != expected:
        raise SystemExit(f"unexpected marker files: {sorted(intervals)}")
    return intervals


def query_for_time(intervals, timestamp):
    matches = [name for name, (start, end) in intervals.items() if start <= timestamp <= end]
    if len(matches) > 1:
        raise SystemExit(f"overlapping query intervals at {timestamp}")
    return matches[0] if matches else None


def trace_reads(trace_path, markers_dir, fixture_root):
    fixture_root = fixture_root.resolve()
    intervals = marker_intervals(markers_dir)
    results = {
        name: {"opens": [], "reads": []}
        for name in ("exact_page", "combined_old", "combined_number")
    }
    offsets = {}
    for line in trace_path.read_text().splitlines():
        timestamp = timestamp_ns(line)
        name = call_name(line)
        if timestamp is None or name is None:
            continue
        query = query_for_time(intervals, timestamp)
        if query is None:
            continue
        if name == "openat":
            path_match = QUOTED_PATH.search(line)
            result = result_number(line)
            if path_match is None or result is None or result < 0:
                continue
            path = path_match.group(1)
            if not path.startswith(str(fixture_root) + "/"):
                continue
            pid = line.split(maxsplit=1)[0]
            offsets[(pid, result)] = 0
            results[query]["opens"].append(relative_path(fixture_root, path))
        elif name == "close":
            fd_match = FD_PATH.search(line)
            if fd_match is None:
                continue
            fd = int(line.split("(", 1)[1].split("<", 1)[0])
            pid = line.split(maxsplit=1)[0]
            offsets.pop((pid, fd), None)
        elif name == "lseek":
            fd_match = FD_PATH.search(line)
            result = result_number(line)
            if fd_match is None or result is None or result < 0:
                continue
            fd = int(line.split("(", 1)[1].split("<", 1)[0])
            pid = line.split(maxsplit=1)[0]
            offsets[(pid, fd)] = result
        elif name in ("read", "pread64"):
            fd_match = FD_PATH.search(line)
            count = result_number(line)
            if fd_match is None or count is None or count <= 0:
                continue
            fd = int(line.split("(", 1)[1].split("<", 1)[0])
            pid = line.split(maxsplit=1)[0]
            if name == "pread64":
                offset = int(line.rsplit(",", 2)[1].strip())
            else:
                offset = offsets.get((pid, fd), 0)
                offsets[(pid, fd)] = offset + count
            results[query]["reads"].append(
                {
                    "path": relative_path(fixture_root, fd_match.group(1)),
                    "offset": offset,
                    "bytes": count,
                }
            )

    for result in results.values():
        result["open_count"] = len(result["opens"])
        result["distinct_paths"] = sorted({path for path in result["opens"]})
        result["read_count"] = len(result["reads"])
        result["bytes_read"] = sum(read["bytes"] for read in result["reads"])
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trace", type=Path, required=True)
    parser.add_argument("--markers-dir", type=Path, required=True)
    parser.add_argument("--fixture-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    results = trace_reads(args.trace, args.markers_dir, args.fixture_root)
    args.output.write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
