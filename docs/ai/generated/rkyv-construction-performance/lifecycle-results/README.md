# Lifecycle and regression evidence

Task: `rkyv-construction-performance`

The final committed implementation was validated with:

| Command or suite | Result |
| --- | --- |
| `cargo test -p objekte-search-core --lib --locked` | 584 passed, 17 ignored, 0 failed |
| `cargo test --workspace --locked --no-fail-fast` | Passed, including CLI and desktop |
| `cargo test -p objekte-search-core --test compaction_api --locked` | Passed |
| deterministic lifecycle matrix | Seeds `1, 7, 19, 41, 97`; worker/cache matrix; fresh rebuild/model checks after every operation |
| `cargo check --workspace --all-targets --locked` | Passed |
| `cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked` | Passed |
| `git diff --check` | Passed |

The focused construction regressions cover:

- empty, singleton, multi-level and 4,096-key ordered posting dictionaries;
- source-order rejection for duplicate, sparse or descending entries;
- one frozen value dictionary with no temporary ordinal tree;
- first-batch ownership transfer without bitmap-member enumeration;
- zero generic-field-container reconstruction during in-place later merges;
- sort configuration with zero payload capacity;
- affected-only metadata and path shard rebasing with ownership preservation;
- relation finalization leaving all reachable base posting dictionaries archived;
- healthy file publication without native file/record cache initialization;
- lazy, shared compatibility row caches and released compatibility lookup maps;
- lightweight empty shells without a construction pool;
- duplicate-ID registry release at construction completion;
- empty archive owner sharing and unique-owner accounting;
- addition, replacement, deletion, relocation and all compaction modes against
  fresh reconstruction and the independent lifecycle model.

The query comparison covered all 150 Musenalm cases through direct and
progressive entry paths for both alphabets. Semantic digest mismatches against
native and predecessor were zero in all four comparisons.
