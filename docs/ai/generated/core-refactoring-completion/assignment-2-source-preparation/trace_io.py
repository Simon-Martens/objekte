#!/usr/bin/env python3

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path


TIMESTAMP = re.compile(
    r"^(?P<pid>\d+)\s+(?P<time>\d+\.\d+)\s+(?P<rest>.*)$"
)
CALL = re.compile(r"^(?P<call>\w+)\(")
RESUMED = re.compile(r"^<\.\.\.\s+(?P<call>\w+)\s+resumed>")
QUOTED_PATH = re.compile(r'"(/[^"]+/data/[^"]+)"')
FD_PATH = re.compile(r"\d+<(/[^>]+/data/[^>]+)>")
RESULT = re.compile(r"\)\s+=\s+(-?\d+)")
PREAD_ARGS = re.compile(r",\s*(-?\d+),\s*(-?\d+)\s*(?:<unfinished|\))")
READ_ARGS = re.compile(r",\s*(-?\d+)\s*(?:<unfinished|\))")


class TraceError(Exception):
    pass


def result_number(text):
    match = RESULT.search(text)
    return None if match is None else int(match.group(1))


def fd_number(text):
    match = re.match(r"\w+\((\d+)<", text)
    return None if match is None else int(match.group(1))


def timestamp_ns(line):
    match = TIMESTAMP.match(line)
    if match is None:
        return None, None
    return int(match.group("pid")), round(float(match.group("time")) * 1_000_000_000)


def relative_path(fixture_root, path):
    return str(Path(path).relative_to(fixture_root))


def marker_intervals(markers_dir):
    intervals = {}
    for path in sorted(markers_dir.glob("*.markers")):
        start, end = path.read_text().split()
        intervals[path.stem] = (int(start), int(end))
    expected = {"exact_page", "combined_old", "combined_number"}
    if set(intervals) != expected:
        raise TraceError(f"unexpected marker files: {sorted(intervals)}")
    return intervals


def query_for_time(intervals, timestamp):
    matches = [name for name, (start, end) in intervals.items() if start <= timestamp <= end]
    if len(matches) > 1:
        raise TraceError(f"overlapping query intervals at {timestamp}")
    return matches[0] if matches else None


def parse_events(lines, fixture_root):
    fixture_root = fixture_root.resolve()
    offsets = {}
    unfinished = {}
    events = []
    for line in lines:
        pid, timestamp = timestamp_ns(line)
        if timestamp is None:
            continue
        text = TIMESTAMP.match(line).group("rest")
        resumed = RESUMED.match(text)
        call_match = CALL.match(text)
        if resumed is not None:
            call = resumed.group("call")
            key = (pid, call)
            if key not in unfinished:
                continue
            original = unfinished.pop(key)
            count = result_number(text)
            if count is None:
                continue
            path_match = FD_PATH.search(text) or QUOTED_PATH.search(text)
            path = original["path"] if path_match is None else path_match.group(1)
            if path is None or not path.startswith(str(fixture_root) + "/"):
                continue
            if call == "openat":
                if count < 0:
                    continue
                offsets[(pid, count)] = 0
                events.append({"timestamp": timestamp, "kind": "open", "path": path})
            elif call == "close":
                fd = original["fd"]
                if fd is not None:
                    offsets.pop((pid, fd), None)
            elif call == "lseek":
                fd = original["fd"]
                if fd is not None and count >= 0:
                    offsets[(pid, fd)] = count
            if call in ("openat", "close", "lseek"):
                continue
            if call == "pread64":
                if count <= 0:
                    continue
                completed = (
                    original["text"].replace("<unfinished ...>", "")
                    + text[resumed.end() :]
                )
                args = PREAD_ARGS.search(completed)
                if args is None:
                    raise TraceError(f"unsupported split pread64 arguments: {completed}")
                offset = int(args.group(2))
            elif call == "read":
                if count <= 0:
                    continue
                offset = offsets.get((pid, original["fd"]), 0)
                offsets[(pid, original["fd"])] = offset + count
            else:
                raise TraceError(f"unsupported split in-scope call: {call}")
            events.append(
                {
                    "timestamp": timestamp,
                    "kind": "read",
                    "path": path,
                    "offset": offset,
                    "bytes": count,
                }
            )
            continue
        if call_match is None:
            continue
        call = call_match.group("call")
        if "<unfinished ...>" in text:
            fd = fd_number(text)
            path_match = FD_PATH.search(text) or QUOTED_PATH.search(text)
            unfinished[(pid, call)] = {
                "fd": fd,
                "path": None if path_match is None else path_match.group(1),
                "text": text,
                "line": line,
                "pid": pid,
            }
            continue

        result = result_number(text)
        if call == "openat":
            path_match = QUOTED_PATH.search(text)
            if path_match is None or result is None or result < 0:
                continue
            path = path_match.group(1)
            if not path.startswith(str(fixture_root) + "/"):
                continue
            offsets[(pid, result)] = 0
            events.append({"timestamp": timestamp, "kind": "open", "path": path})
        elif call == "close":
            fd = fd_number(text)
            if fd is not None:
                offsets.pop((pid, fd), None)
        elif call == "lseek":
            fd = fd_number(text)
            if fd is not None and result is not None and result >= 0:
                offsets[(pid, fd)] = result
        elif call in ("read", "pread64"):
            path_match = FD_PATH.search(text)
            fd = fd_number(text)
            if path_match is None or result is None:
                continue
            path = path_match.group(1)
            if not path.startswith(str(fixture_root) + "/"):
                continue
            if result <= 0:
                continue
            if call == "pread64":
                args = PREAD_ARGS.search(text)
                if args is None:
                    raise TraceError(f"unsupported pread64 arguments: {line}")
                offset = int(args.group(2))
            else:
                args = READ_ARGS.search(text)
                if args is None:
                    raise TraceError(f"unsupported read arguments: {line}")
                offset = offsets.get((pid, fd), 0)
                offsets[(pid, fd)] = offset + result
            events.append(
                {
                    "timestamp": timestamp,
                    "kind": "read",
                    "path": path,
                    "offset": offset,
                    "bytes": result,
                }
            )
        elif path_match := (FD_PATH.search(text) or QUOTED_PATH.search(text)):
            path = path_match.group(1)
            if path.startswith(str(fixture_root) + "/"):
                raise TraceError(f"unsupported in-scope operation {call}: {line}")
    if unfinished:
        raise TraceError(f"unfinished in-scope calls: {sorted(unfinished)}")
    return events


def extract_results(lines, markers_dir, fixture_root):
    fixture_root = fixture_root.resolve()
    intervals = marker_intervals(markers_dir)
    results = {
        name: {"opens": [], "reads": []}
        for name in ("exact_page", "combined_old", "combined_number")
    }
    for event in parse_events(lines, fixture_root):
        query = query_for_time(intervals, event["timestamp"])
        if query is None:
            continue
        path = relative_path(fixture_root, event["path"])
        if event["kind"] == "open":
            results[query]["opens"].append(path)
        else:
            results[query]["reads"].append(
                {
                    "path": path,
                    "offset": event["offset"],
                    "bytes": event["bytes"],
                }
            )
    for result in results.values():
        result["open_count"] = len(result["opens"])
        result["distinct_paths"] = sorted(set(result["opens"]))
        result["read_count"] = len(result["reads"])
        result["bytes_read"] = sum(read["bytes"] for read in result["reads"])
    return results


def self_test():
    lines = [
        '10 1.100000 openat(AT_FDCWD, "/fixture/data/00.json", O_RDONLY) = 3</fixture/data/00.json>',
        '10 1.100001 pread64(3</fixture/data/00.json>, "hij", 3, 7) = 3',
        '10 1.100002 lseek(3</fixture/data/00.json>, 4, SEEK_SET) = 4',
        '10 1.100003 read(3</fixture/data/00.json>, "abc", 3) = 3',
        '10 1.100004 openat(AT_FDCWD, "/fixture/data/01.json", O_RDONLY <unfinished ...>',
        '10 1.100005 <... openat resumed>) = 4</fixture/data/01.json>',
        '10 1.100006 read(4</fixture/data/01.json>, "abc"..., 3 <unfinished ...>',
        '10 1.100007 <... read resumed>"abc", 3) = 3',
    ]
    with tempfile.TemporaryDirectory(prefix="objekte-trace-self-test-") as directory:
        root = Path(directory)
        data = root / "data"
        markers = root / "markers"
        data.mkdir()
        markers.mkdir()
        (markers / "exact_page.markers").write_text("1000000000 1200000000\n")
        (markers / "combined_old.markers").write_text("1300000000 1400000000\n")
        (markers / "combined_number.markers").write_text("1500000000 1600000000\n")
        results = extract_results(lines, markers, Path("/fixture"))
    exact = results["exact_page"]
    if exact["reads"] != [
        {"path": "data/00.json", "offset": 7, "bytes": 3},
        {"path": "data/00.json", "offset": 4, "bytes": 3},
        {"path": "data/01.json", "offset": 0, "bytes": 3},
    ]:
        raise TraceError(f"trace self-test mismatch: {exact['reads']}")
    if [read["bytes"] for read in exact["reads"]].count(3) != 3:
        raise TraceError("trace self-test lost a split call occurrence")
    split_pread64 = [
        (
            'pread64(3</fixture/data/00.json>,  <unfinished ...>',
            '"hi", 9, 7) = 2',
        ),
        (
            'pread64(3</fixture/data/00.json>, "hi", 9, <unfinished ...>',
            '7) = 2',
        ),
        (
            'pread64(3</fixture/data/00.json>, "hi", 9, 7 <unfinished ...>',
            ') = 2',
        ),
    ]
    for before, after in split_pread64:
        events = parse_events(
            [
                f"11 1.100010 {before}",
                f"11 1.100011 <... pread64 resumed>{after}",
            ],
            Path("/fixture"),
        )
        reads = [event for event in events if event["kind"] == "read"]
        expected_read = {
            "timestamp": 1_100_011_000,
            "kind": "read",
            "path": "/fixture/data/00.json",
            "offset": 7,
            "bytes": 2,
        }
        if reads != [expected_read]:
            raise TraceError(f"split pread64 mismatch: {reads}")
    try:
        parse_events(
            ['10 1.100000 mmap(NULL, 10, PROT_READ, MAP_SHARED, 3</fixture/data/00.json>, 0) = 1'],
            Path("/fixture"),
        )
    except TraceError:
        pass
    else:
        raise TraceError("unsupported in-scope operation was silently accepted")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trace", type=Path)
    parser.add_argument("--markers-dir", type=Path)
    parser.add_argument("--fixture-root", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        print("trace extraction self-tests passed")
    if args.trace is None:
        return 0
    if args.markers_dir is None or args.fixture_root is None or args.output is None:
        raise TraceError("--trace requires --markers-dir, --fixture-root and --output")
    results = extract_results(
        args.trace.read_text().splitlines(),
        args.markers_dir,
        args.fixture_root,
    )
    args.output.write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps(results, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except TraceError as error:
        print(f"trace extraction failed: {error}", file=sys.stderr)
        raise SystemExit(1)
