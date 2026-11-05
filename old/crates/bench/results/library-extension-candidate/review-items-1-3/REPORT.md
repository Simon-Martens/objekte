# Review items 1–3: correctness and regression handoff

## Revisions and scope

- Starting revision: `e88a91cc49c00bd0798a271df4a81ce0ac1de67f` on `library-extensions-stage2-wip`.
- Final production revision: `1a520f6d2f5fc29918d7f2ff30bf0b80d43b0ba7`.
- The starting checkout also contained the user-owned `docs/impl-plan.md` and modified `paused-stage2/BLOCKER.md`; neither was changed or committed here. The [manifest](manifest.json) records the starting status, compiler, lockfile, workload, binary digests and source fingerprint.
- Only review items 1–3 were implemented. Custom file-type integration was not started.

The custom text search now requires `can_skip_non_candidates()` before applying a custom bigram constraint, as the logical frontend already did. The representation and preparation checks remain in place. Completed ordered-cache hits copy only the requested page of slots, then resolve each slot through per-collection source tables and prefix offsets. Cacheability and identity now use the bound frontend, operators, representations and derived extractors, applicable source rules and decoder dispatch, schema mappings and a separate selected-source generation. Selected posting keys use the same scoped definitions. Selected representation manifests moved to version 2 and selected derived snapshots to `OBJDRV02`; older metadata is a cache miss.

## Benchmark method and results

The workload file contains **152** loaded cases. The Musenalm corpus contained 148,486 JSON records and had the same content/stat fingerprint (`90e36ad63dcb6386e9521c583fdcd36451fd3d6182187dced542fb8292f730ce`) before and after. Both runs used Rust/Cargo 1.99.1, normal release build, configured schemas, ASCII bigrams, `RAYON_NUM_THREADS=16`, CPU affinity `0-15`, seven measured rounds and the existing warm-index preparation. The 64 MiB requested budget resulted in the same reported internal query-cache budget of 6,682,311 bytes in both builds; the zero-budget runs reported zero.

The reference executable was copied to the ignored `candidate-bin/reference/` directory before production changes. The candidate executable was copied to `candidate-bin/candidate/` after the final production change. The exact commands, exit statuses and invocation times are in the [reference](reference/commands.json) and [candidate](candidate/commands.json) command logs. Each phase used:

```sh
cargo build --locked --release -p objekte-bench --bin json-query-mix -j 2
env RAYON_NUM_THREADS=16 taskset -c 0-15 python3 crates/bench/run_library_query_matrix.py \
  --bin-dir crates/bench/results/library-extension-candidate/review-items-1-3/candidate-bin/reference \
  --output-dir crates/bench/results/library-extension-candidate/review-items-1-3/reference \
  --runs 7 --only json-schema-ascii-cache0,json-schema-ascii-cache64
env RAYON_NUM_THREADS=16 taskset -c 0-15 python3 crates/bench/run_library_query_matrix.py \
  --bin-dir crates/bench/results/library-extension-candidate/review-items-1-3/candidate-bin/candidate \
  --output-dir crates/bench/results/library-extension-candidate/review-items-1-3/candidate \
  --runs 7 --only json-schema-ascii-cache0,json-schema-ascii-cache64
```

The runner performed one invocation per binary and budget. It validated one setup row, 152 case rows and seven samples per case; all four benchmark processes exited successfully. The raw outputs and stderr are under [reference](reference/) and [candidate](candidate/). [Per-query results](per-query.csv) give all 304 aligned cases with both medians, min/max spread, absolute and relative deltas, signatures and source-work counters. [Machine summary](comparison.json) records the largest increases and the material-slowdown screen. The aggregate below sums case medians; it is not process wall time.

| Requested cache | Reference sum | Candidate sum | Delta | Largest individual increase | Signature/work differences |
|---|---:|---:|---:|---|---|
| 64 MiB | 9,915.36 ms | 9,978.64 ms | +63.28 ms (+0.64%) | Case 117: +8.30 ms (+2.14%) on 388.69 ms | 0 / 152 |
| 0 MiB | 10,836.57 ms | 10,902.97 ms | +66.40 ms (+0.61%) | Case 117: +8.68 ms (+2.25%) on 385.52 ms | 0 / 152 |

No case showed a plausible material slowdown in this pair: the largest positive deltas were below 9 ms and below 3% on queries taking hundreds of milliseconds. No targeted alternating repeats were triggered. Every case retained the same match count, first ID, candidate scans, source reads, parses and cache-hit counters. The benchmark signatures cover counts and first IDs; the focused tests below verify complete page ordering and errors.

## Correctness checks

- `cargo test --locked --workspace --all-targets -j 2` and `cargo clippy --locked --workspace --all-targets -j 2 -- -D warnings` passed; see [test log](workspace-tests.log) and [Clippy log](clippy.log). The core library had 137 unit tests and the external consumer had 28 tests.
- The extended fallible-operator test exercises logical, collecting and streaming entry points with cache budgets 0 and 64 MiB, repeated failures, and AND/OR short-circuit behavior. The safe prefix branch still uses candidate acceleration.
- The new paging test checks first, middle, last, empty and large-offset pages across two collections, page-only reads/parses, edits, insertions, deletions, refresh and zero/tiny cache budgets. Core unit tests check direct large-slot lookup and that a 100,000-slot ordered entry copies only a one-slot requested page.
- The dependency test checks unused volatile operator/extractor cache hits, used volatile nonreuse, used version changes, stable identity under unrelated changes and registration reorder, and predicate/sort key separation. An initially empty applicable provider and decoder version participate in selection identity; adding a Markdown source and refreshing invalidates the old result.
- `python3 scripts/format-rust-spacing.py --check` still fails only on the same seven pre-existing formatting hunks in untouched files named in the earlier final report; see [spacing log](spacing-check.log). Changed Rust files were rustfmt- and spacing-formatted. `git diff --check` passed.

## Invariants and limits for the next task

The `MetadataOnOpen` source table and its generation remain reusable until explicit refresh/reload; completed cache entries hold slot numbers only. A hit must retain page-size slot copying and page-only read/decode work, with no full inventory flattening. Refresh must replace the selected table and invalidate result/posting keys before old slots can be interpreted against new sources. `MetadataOnRequest` and `HashOnRequest` retain their existing collection validation cost and freshness behavior. A candidate hint never suppresses a fallible operator unless it explicitly promises safe skipping, including through Boolean plans. Applicable provider rules remain dependencies even when empty. Because decoder `accepts` may inspect locator or metadata and has no invariant applicability declaration, all registered decoder descriptors are conservatively included when a custom provider can select that collection.

This comparison covers the agreed Musenalm JSON workload only. It does not measure custom frontend latency, allocations, other corpora or cold physical storage; those were excluded by the review instruction. It supports correctness parity and no confirmed material regression for the recorded cases, not universal zero overhead.
