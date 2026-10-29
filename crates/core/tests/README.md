# Verification

Run from the repository root:

```sh
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
python3 scripts/format-rust-spacing.py --check
```

The routine core suite covers configuration, source safety, JSON and filter semantics, view ordering/pagination, references, and segment-cache updates. `segment_cache.rs` tests targeted edits, deletion/compaction, revision persistence, all-field scalar indexing, configuration changes, on-demand validation, cache corruption/version recovery, and concurrent builders. `views.rs` includes 60 deterministic memory and segment mutation steps checked against direct source execution. `json_filter.rs` compares the default bigram, segment, direct, and raw-file grep backends across nested paths, arrays, exact numbers, Unicode escapes, parameters, typed `OR` unions, and safe fallback when an `OR` branch cannot be indexed. CLI tests use real subprocesses and a shared cache lock.

The two Musenalm acceptance tests are opt-in because they read all 148,486 example records:

```sh
cargo test -p objekte-core --release --test musenalm -- --ignored --nocapture
cargo test -p objekte-core --release --test musenalm_json_filter -- --ignored --nocapture
```

Historical engine benchmarks are in `crates/bench/` and are separate from the current cache implementation.
