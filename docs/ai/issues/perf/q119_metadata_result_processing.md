# Q119 metadata and result-processing cost

- Category: `perf`
- Originating task: `index-array-length-repair`
- Branch and implementation commit: `master`, `90b52907c7f9`
- Relevant code: `crates/core/src/executor.rs` metadata access path and `crates/core/src/run.rs` count/page execution.

## Observation

The unchanged Musenalm Q119 predicate, `@file.modified >= "2026-01-01T00:00:00Z"`, matches 133,948 records. The three-repetition final full suite measured 26.748 ms on base `95f1736d448a` and 27.294 ms on `90b52907c7f9` in Unicode cache-off mode. Its query opens zero source files, reads zero source bytes, parses zero records, and performs zero source scans in both revisions. The selected exact metadata path visits 133,948 candidates; median planning time is about 7.3 ms. The remaining time is metadata membership, count, and page processing.

Reproduce with `target/release/examples/bench_musenalm --root examples/musenalm --extended --cases Q119 --profiles ascii-off,unicode-off --repetitions 7 --query-workers 4 --verified-cache-bytes 0 --result-dir <new-result-directory>`. Measurements and plans from the originating task are in `bench/results/structural-predicate-index-repair/2026-11-11/`.

ASCII timing varied: 26.363 ms before and 26.898 ms after in the final full suite. A seven-repetition paired control measured 33.290 ms before and 38.350 ms after; an eleven-repetition reverse-order control measured 26.081 ms before and 28.146 ms after. Unicode medians in those controls moved from 34.657 to 36.514 ms and from 25.998 to 26.649 ms, respectively. The same access path, candidate count, and source-work counters held throughout. These timings do not isolate a redundant local operation or establish a stable repair-induced regression.

## Expected behavior and follow-up

Q119 is correct and source-free, but roughly 26–38 ms in these runs is substantial for a metadata-only query. A separate performance investigation could profile the metadata bitmap construction and count/page path under controlled CPU and cache conditions, then compare a targeted index or vectorized metadata predicate if the profile warrants it. Those approaches are hypotheses; this task did not authorize a metadata-index redesign. No Q119 code change was made.
