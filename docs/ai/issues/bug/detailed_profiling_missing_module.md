# `detailed-profiling` build references a missing `profiling` module

> Resolved by `core-index-construction` on 2026-11-18: both references now use
> `crate::detailed_timer::DetailedInstant`, and the all-feature check passes.

- Category: bug
- Originating task: `lifecycle-publication-and-read-views`
- Observed on: assignment base `0a0e0c3507af94164fce2387070cfcb6b5277d23` and tested implementation `b14234cf5cc8ef9ac8beb20e05d3b9a0e0efe774`
- Code locations: `crates/core/src/query_results/cache.rs:362` and `crates/core/src/query_results/cache.rs:436`

## Observed behavior

Running

```sh
cargo check --locked -p objekte-search-core --all-targets --all-features
```

enables the `detailed-profiling` feature and fails with two `E0433` errors because `crate::profiling::DetailedInstant` cannot be resolved. The crate root has no `profiling` module.

## Expected behavior

The core crate should compile with all features enabled.

## Reproduction

The failure reproduces both on untouched assignment base `0a0e0c3507af94164fce2387070cfcb6b5277d23` and on the lifecycle integration branch, so it is unrelated to the lifecycle module extraction.

```sh
cargo check --locked -p objekte-search-core --all-targets --all-features
```

## Impact and scope

This prevents the lifecycle task's all-feature validation command from passing. The ordinary all-target check, full core test suite and CLI check pass. The lifecycle task did not change `query_results/cache.rs` because its scope is a mechanical lifecycle-only refactor.

## Possible approaches

Compiler diagnostics identify `crate::detailed_timer::DetailedInstant` as an available type. The profiling code may have moved or been renamed without updating these two references. Before changing it, confirm whether `detailed_timer::DetailedInstant` is the intended implementation for the `detailed-profiling` feature and whether a compatibility re-export named `profiling` is required elsewhere.
