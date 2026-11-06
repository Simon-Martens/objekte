# Review item 4: ordinary selected-source integration

## Revision and implementation

Starting revision: `ed91c70da6862bd786a4a6493117b6138a0e114b` on `library-extensions-stage2-wip`. It differs from the measured first-task production revision `1a520f6d2f5fc29918d7f2ff30bf0b80d43b0ba7` only in committed benchmark/report files. Implementation revision: `f367db9383feea8a08a8a9cac415d6b44ec6a3fe`. The pre-existing modified `paused-stage2/BLOCKER.md` and untracked `docs/impl-plan.md` were left untouched. [Baseline manifest](baseline-manifest.json) records the exact binary, toolchain, workload, corpus and starting worktree.

The narrow boundary is the engine-owned selected `SourceDescriptor` inventory in `extension_sources.rs`. Ordinary JSON/text queries, their streaming entry points, views, record access, relation traversal, validation and derived queries route through it only when a scope contains custom records. JSON-only scopes keep the native picker and query executors. Custom records are read through their provider and decoder using the actual locator; the owned logical tree is constructed only for ordinary result/evaluation APIs, while selected index extraction retains callback-scoped borrowed access. Record-ID collisions fail explicitly. The [implementation map](IMPLEMENTATION-MAP.md) records the previous boundary.

Selected text and derived postings now participate in ordinary warm/build/save/reopen. The repository saves a version-1 selected manifest, restores compatible selected text companion manifests (version 2) and derived postings (`OBJDRV02`), and rebuilds incompatible or corrupt postings. Native JSON snapshot bytes remain unchanged. Refresh rebuilds active selected postings before exposing the new selected generation; a failed rebuild makes that collection unavailable until a successful refresh or reload. Cache keys include the selected source generation and bound dependencies. A completed ordered hit slices the requested page and resolves slots against the pinned inventory. `MetadataOnRequest` and `HashOnRequest` retain their inventory checks. The default JSON route adds no extension dispatch per record.

## Correctness checks

`cargo test --locked --workspace --all-targets -j 2` and `cargo clippy --locked --workspace --all-targets -j 2 -- -D warnings` passed. Changed Rust files pass targeted rustfmt. Core `index-timings` and bench `allocation-profile` feature checks pass. Core doctests pass. The repository-wide spacing check still fails only in the unchanged baseline files `bigram/filter.rs`, `config/discovery.rs`, `extensions.test.rs`, and `tests/index_build_profile.rs`; no changed file is reported.

The external consumer crate has 32 passing tests. New focused coverage checks custom-only and mixed ordinary JSON/text access, streaming, views, relations, null/missing/array values, sort ties, complete pages and cached page-only reads; Markdown/HTML/XML matching; a representation index that skips 31 of 32 records; fallible and safe operator candidate behavior; ordinary derived indexing and value counts; save/reopen restoration without source reads; corrupt and version-mismatched snapshot rebuilding; provider edits, insertions, deletions, renames and initially empty selection; failed refresh and decoder errors. The mixed lifecycle test compares complete results after changes against an exact logical-data oracle.

## Agreed 152-case regression comparison

The starting binary was preserved separately (`3cdef321ed8c8257d742fb6784a6377a8ac400bfe687d69fd4a385883f45144f`); the delivered-code release binary has SHA-256 `fa5bfe1390ca6d404da66c67871d8b05345ce1fd1e94eb446ab611ce8b096c0e`. Both used Rust/Cargo 1.99.1, normal release flags, configured schemas, ASCII bigrams, 16 Rayon threads, CPU affinity `0-15`, the same warm-index preparation, and the same 148,486-record Musenalm source/content/stat fingerprint `90e36ad63dcb6386e9521c583fdcd36451fd3d6182187dced542fb8292f730ce`. The workload contains **152 loaded cases**, SHA-256 `5cfb37386221a6f2237bc1755e12c7a89f1e1f4fdeff4dd57f1b00370c4eb99e`. The saved first-task invocation was reused as reference because revision, binary, host, inputs and settings matched; its original command and timestamps are in [reference/commands.json](reference/commands.json).

Candidate command (one invocation per budget; the runner invokes the binary with `--repo examples/musenalm --runs 7 --bigram-encoding ascii --query-cache-mib {0,64}`):

```sh
cargo build --locked --release -p objekte-bench --bin json-query-mix -j 2
install -D -m755 target/release/json-query-mix crates/bench/results/library-extension-candidate/review-item-4/candidate-bin/candidate/json-query-mix
env RAYON_NUM_THREADS=16 taskset -c 0-15 python3 crates/bench/run_library_query_matrix.py \
  --bin-dir crates/bench/results/library-extension-candidate/review-item-4/candidate-bin/candidate \
  --output-dir crates/bench/results/library-extension-candidate/review-item-4/candidate \
  --runs 7 --only json-schema-ascii-cache0,json-schema-ascii-cache64
python3 crates/bench/results/library-extension-candidate/review-item-4/compare.py
```

| Query cache | Sum of 152 case medians, reference → candidate | Change | Largest positive case delta |
| --- | ---: | ---: | ---: |
| 64 MiB | 9,978.64 → 9,941.89 ms | −36.75 ms (−0.37%) | case 110: +18.22 ms (+3.28%) |
| 0 MiB | 10,902.97 → 10,808.85 ms | −94.12 ms (−0.86%) | case 120: +1.60 ms (+0.29%) |

All 304 case/budget result signatures and recorded scans, reads, parses and cache-hit counts match. Both signature files have SHA-256 `9927abc9049cb2f6243bd8066fe66196beed448454d42abb587f2f10b7be0c96`. [Per-query results](per-query.csv) include every median, sample min/max, absolute and relative delta, signature and work count; [raw candidate output](candidate/) and [comparison summary](comparison.json) are retained.

Case 110 at 64 MiB was repeated in three alternating reference/candidate pairs with seven rounds each, using the single-case [workload](case110-targeted.json). Pairwise median changes were −4.19 ms (−0.79%), −12.79 ms (−2.38%) and +1.32 ms (+0.25%); source work and signatures matched. The original +18.22 ms observation is not reproducible. [Repeat samples](case110-repeat/summary.json) and raw process outputs are retained. No confirmed material regression remains in the agreed workload. Seven samples do not establish tail latency.

## Limits and next-step invariants

The selected refresh currently rebuilds active custom postings for the changed collection instead of patching each posting. Mixed ordinary JSON scalar filters and sorts use exact scanning; native `build_posting` and picker `value_counts` remain JSON-only, with mixed `value_counts` returning `None` instead of an incomplete answer. Low-level `diff`, `apply_source_changes`, `record_revision` and schema inference remain picker-specific. Custom streaming routes deliver a bounded computed page through the streaming callback. The agreed benchmark protects existing JSON queries; it does not measure custom-format throughput.

Future changes must keep each selected source table, text/typed posting and ordered slot sequence on the same source generation; include applicable provider/decoder selection rules and bound operation dependencies in cache identities; never publish partial indexes or complete cache entries after an error; preserve native secure reads and optimized JSON paths; and make a completed ordered hit read/decode only its returned page.
