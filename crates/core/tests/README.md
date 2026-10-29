# Verification

Run from the repository root:

```sh
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
python3 scripts/format-rust-spacing.py --check
```

The routine core suite covers configuration, source safety, JSON and filter semantics, view ordering/pagination, references, and segment-cache updates. `segment_cache.rs` tests targeted edits, deletion/compaction, revision persistence, all-field scalar indexing, configuration changes, on-demand validation, cache corruption/version recovery, and concurrent builders. `views.rs` includes 60 deterministic memory and segment mutation steps checked against direct source execution. `json_filter.rs` compares the default bigram, segment, direct, and raw-file grep backends across nested paths, arrays, exact numbers, Unicode escapes, parameters, typed `OR` unions, and safe fallback when an `OR` branch cannot be indexed. CLI tests use real subprocesses and a shared cache lock.

`json_query_oracle.rs` has a routine fixture test that scans source files with `serde_json` and handwritten predicates, then checks complete IDs against all four backends. Its opt-in Musenalm test applies an independent predicate to every one of the 43 benchmark filters and checks complete IDs against bigram and segments. `json_query_limit.rs` checks bounded parallel page scanning and full scans for counts or global cardinality errors.

`json_query_stream.rs` compares streamed pages and metadata against collected queries on all four backends, and checks that a late scalar-cardinality error invalidates previously emitted records. CLI subprocess tests verify numbered provisional events, final metadata, and a late error event.

The two Musenalm acceptance tests are opt-in because they read all 148,486 example records:

```sh
cargo test -p objekte-core --release --test musenalm -- --ignored --nocapture
cargo test -p objekte-core --release --test musenalm_json_filter -- --ignored --nocapture
cargo test -p objekte-core --release --test json_query_oracle -- --ignored --nocapture
```

Historical engine benchmarks are in `crates/bench/` and are separate from the current cache implementation.
