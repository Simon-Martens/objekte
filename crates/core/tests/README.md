# Verification

Run from the repository root:

```sh
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
python3 scripts/format-rust-spacing.py --check
```

The routine core suite covers configuration, source safety, JSON filter semantics, view ordering and pagination, references, in-memory index refresh, and explicit warm and rebuild operations. `views.rs` includes 60 deterministic memory mutation steps checked against direct source execution. `json_filter.rs` compares bigram, direct, and raw-file grep backends across nested paths, arrays, exact numbers, Unicode escapes and case folding, parameters, typed `OR` unions, and safe fallback when an `OR` branch cannot be indexed.

`json_query_oracle.rs` uses `serde_json` and handwritten predicates to compare complete IDs with the query backends. Its opt-in Musenalm test checks all 43 benchmark filters against the in-memory bigram backend. `json_query_limit.rs` checks bounded parallel page scanning and full scans for counts or global cardinality errors.

`json_query_stream.rs` compares streamed pages and metadata with collected queries and checks that a late scalar-cardinality error invalidates previously emitted records. CLI subprocess tests verify numbered provisional events, final metadata, and a late error event.

The Musenalm acceptance tests are opt-in because they read the full example collection:

```sh
cargo test -p objekte-core --release --test musenalm -- --ignored --nocapture
cargo test -p objekte-core --release --test musenalm_json_filter -- --ignored --nocapture
cargo test -p objekte-core --release --test json_query_oracle -- --ignored --nocapture
```

Historical engine benchmarks are under `crates/bench/` and describe earlier implementations.
