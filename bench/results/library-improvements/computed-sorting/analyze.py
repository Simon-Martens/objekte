#!/usr/bin/env python3
"""Check paired Q4 raw runs and print median query times in milliseconds."""

import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def load(path: Path):
    rows = [json.loads(line) for line in path.read_text().splitlines()]
    run = rows[0]
    assert run["kind"] == "run" and run["profile"] == "release"
    builds = {(r["layout"], r["reuse"], r["workers"]): r for r in rows if r["kind"] == "build"}
    queries = defaultdict(list)
    for row in rows:
        if row["kind"] == "query":
            queries[(row["layout"], row["reuse"], row["workers"])].append(row)
    assert builds.keys() == queries.keys()
    for key, samples in queries.items():
        assert len(samples) == run["repetitions"] + 1, (path, key)
        assert [r["repetition"] for r in samples] == list(range(run["repetitions"] + 1))
        assert all(r["count"] == run["rows"] for r in samples)
        assert len({r["answer_sha256"] for r in samples}) == 1
        assert all(r["sha"] == run["sha"] for r in samples)
    return run, builds, queries


def main():
    if len(sys.argv) != 5:
        raise SystemExit("usage: analyze.py baseline-local final-local baseline-related final-related")
    for left_path, right_path in zip(map(Path, sys.argv[1::2]), map(Path, sys.argv[2::2])):
        before, before_builds, before_queries = load(left_path)
        after, after_builds, after_queries = load(right_path)
        assert before["rows"] == after["rows"]
        assert before["repetitions"] == after["repetitions"]
        assert before["mode"] == after["mode"]
        assert before_builds.keys() == after_builds.keys()
        print(f"{before['mode']}: {before['sha'][:12]} -> {after['sha'][:12]}")
        for key in sorted(before_builds):
            old_build, new_build = before_builds[key], after_builds[key]
            assert old_build["fixture_sha256"] == new_build["fixture_sha256"], key
            old_samples, new_samples = before_queries[key], after_queries[key]
            assert old_samples[0]["answer_sha256"] == new_samples[0]["answer_sha256"], key
            assert [
                (r["sort_target_reads"], r["sort_target_reuse"]) for r in old_samples
            ] == [
                (r["sort_target_reads"], r["sort_target_reuse"]) for r in new_samples
            ], key
            old_ms = [r["elapsed_ns"] / 1_000_000 for r in old_samples[1:]]
            new_ms = [r["elapsed_ns"] / 1_000_000 for r in new_samples[1:]]
            old_median = statistics.median(old_ms)
            new_median = statistics.median(new_ms)
            print(
                f"  {key[0]:4} {key[1]:4} workers={key[2]} "
                f"base={old_median:.3f} [{min(old_ms):.3f},{max(old_ms):.3f}] "
                f"final={new_median:.3f} [{min(new_ms):.3f},{max(new_ms):.3f}] "
                f"ratio={old_median / new_median:.2f}x "
                f"target_reads={new_samples[0]['sort_target_reads']} "
                f"reuse={new_samples[0]['sort_target_reuse']}"
            )


if __name__ == "__main__":
    main()
