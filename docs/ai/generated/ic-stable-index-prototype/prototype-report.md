# Independent stable-index prototype report

Task: `ic-stable-index-prototype`  
Date: 16 November 2026  
Base commit: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`  
Integration branch: `integration/ic-stable-index-prototype`  
Integration worktree: `/home/simon/source/objekte-integration-ic-stable-index-prototype`

## Decision

**Performance rejection.** The storage design, independent component separation,
standalone save/open, lazy generation loading, mutation, and semantic validation
work. Construction latency is outside the required gate:

- P=8 matched median: **4.617 s candidate vs 1.637 s native**, ratio **2.82x**.
- The independent component construction stage alone took **3.001 s**, already
  **1.83x** the complete native build. Removing the temporary native build would
  therefore not by itself reach the `<= 1.10x` target.
- Resident and peak memory were at parity (`0.99x` in the matched run).

The result is not presented as a near-parity prototype. No production migration,
native index removal or master merge is implied.

## Implemented prototype

New feature: `ic-stable-prototype`, optional dependency
`ic-stable-structures = "=0.7.2"`. Default builds are unchanged.

Each component owns one `RefCell<Vec<u8>>` and one `BTreeMap<ComponentKey,
StoredBytes>`. Components are independent memories and independent files. No
global memory manager or shared backing buffer is used.

Implemented families:

- physical-file catalog, path lookup, per-collection record catalog,
  record-identity lookup and live/logical/raw/empty-array domains;
- typed values, lengths and structural states per declared field and named
  binding;
- normal and skip bigrams per text binding;
- configured ascending and descending sort components;
- relation occurrences, target postings and relation states;
- explicit component statistics and build-time per-component trace.

Each component file is self-contained: 128-byte envelope, bounded embedded
descriptor, SHA-256 descriptor digest, payload length, entry count, payload
SHA-256, pinned crate/layout identity, then the stable memory image. The reader
uses `BTreeMap::load`, never reinserts entries, and rejects other tree layouts.
Manifest files group a generation but are not required to open one component.

Standalone operations implemented:

- `StablePrototype::save_index(component, new_file)`;
- `StablePrototype::open_index(file)`;
- `StandaloneIndex::descriptor()`;
- `StandaloneIndex::save_as(new_file)`;
- raw lookup/range/insert/replace/remove/posting operations with component-kind
  key checking;
- `live_posting` returns `MissingDependency` instead of pretending standalone
  slots are live.

Snapshot operations implemented:

- `save_snapshot(new_generation_directory)`;
- `open_snapshot(generation_directory)`;
- `load_component(component_id)`;
- unchanged unloaded components are stream-copied without decoding;
- loaded components use `into_memory()` and are restored over the same
  allocation even if writing fails;
- staging directory publication by same-filesystem rename, file and directory
  sync where supported.

Mutation implemented:

- known-key insert, replacement and removal on reopened stable trees;
- batched posting update: decode once, apply additions/removals, encode once;
- append-only file add, logical file delete (live-slot tombstone), replacement
  and move (move rejects path-derived identities unless replacement is used);
- old generations remain readable after later in-memory mutation and saves.

## Correctness and tests

25 focused prototype tests cover:

- native-equivalent catalogs, identities, values, lengths, states, grams, sorts
  and relations;
- empty repository, multiple collections sharing one physical path, non-UTF-8
  Unix paths, duplicate identity slots, dangling/resolved relations;
- standalone save/copy/rename/open/mutate/save-as without any manifest;
- generation lazy loading, independent member opening, second generation, old
  generation survival;
- unchanged-unloaded component copy without load;
- field query loading only its field dictionary and live domain;
- known-key mutation after reopen and second save;
- truncated file, flipped payload byte, bad magic, descriptor flip, oversized
  descriptor, wrong component ID and wrong key-kind rejection;
- failed snapshot destination preserving the previous generation.

Validation compares exact memberships and metadata against independently
constructed native repository structures, not only counts.

## Verification commands

All of the following were run in the integration worktree:

```text
cargo check --workspace --all-targets
cargo test --workspace
cargo check -p objekte-search-core --all-targets --features ic-stable-prototype
cargo test -p objekte-search-core --features ic-stable-prototype
cargo check -p objekte-search-core --all-targets --features ic-stable-prototype,detailed-profiling
cargo test --workspace --features objekte-search-core/ic-stable-prototype
```

Outcomes:

- workspace check passed;
- core feature and profiled-feature checks passed;
- core feature tests passed, including doc tests;
- workspace feature tests passed;
- one full workspace run hit an unrelated desktop test
  (`export_uses_current_valid_draft_before_debounce_runs`) that passed when
  rerun alone and in the subsequent full workspace feature run.

## Benchmark environment

- Linux, AMD Ryzen 7 5700G, 8 cores / 16 hardware threads, 16 logical CPUs.
- Affinity: CPUs 0-15.
- Release build, warm filesystem cache for both sides.
- Corpus: `examples/musenalm`, ASCII bigrams, 148,486 files/records,
  193,297,230 source bytes.
- Schema manifest digest:
  `38f2dc5112146aa6ac0514291c76eaeb741421fa8a4624e50fa83c4c0a449bf9`.
- Baseline: pristine `bench_build_musenalm` executable from the pinned base
  checkout `/home/simon/source/objekte-baseline-ic-stable-index-prototype`.
- Candidate: `bench_ic_stable_index --mode source-build`.

### Exploratory worker counts

| Workers | Native build | Candidate total | Ratio |
| ---: | ---: | ---: | ---: |
| 1 | 6.157 s | 16.539 s | 2.69x |
| 2 | 3.828 s | 7.575 s | 1.98x |
| 8 | 1.637 s | 4.617 s | 2.82x |

P=8 is the selected matched configuration. Five alternating fresh-process
samples gave:

- baseline median **1.637 s**, range 1.600-1.682 s;
- candidate median **4.617 s**, range 4.556-4.658 s;
- candidate component-transfer stage median **3.001 s**, range 2.967-3.016 s;
- median total ratio **2.82x**.

The candidate trace contains 1,323 component jobs. In the diagnostic run, 7,145
pairs of field-component jobs overlapped on different workers, so independent
field construction is real rather than only configured thread count. Largest
tail components were configured sort trees (`preferred_title` and `numbering`)
around 1.0 s each.

### Memory

Matched P=8 run:

- native resident **397.3 MB**, peak **410.3 MB**;
- candidate resident **393.9 MB**, peak **407.2 MB**;
- ratios approximately **0.99x** for both;
- total owned stable component payload **403.0 MB**.

### Persistence

On the full Musenalm generation:

- snapshot save of 1,323 components / ~403 MB: **0.54 s**;
- manifest open without payload loading: **27 ms**;
- first load of one 27.9 MB typed-value component: **27 ms**;
- standalone `open_index` of the copied component: **28 ms**;
- standalone `save_as`: about **36 ms** for that component;
- component load counters report zero insertion calls.

### Primitives and mutation

On the 133,948-key `musenalm_id` value component:

- resident exact posting lookups: 0.005-0.276 ms after first load;
- first load plus live-domain dependency: 52 ms;
- standalone mutation batch including a 65,536-member bitmap and a small
  replacement: 36 ms;
- save of the mutated 27.9 MB standalone component: 36 ms;
- reopen succeeded with one additional key and the descriptor marked detached.

## Reproduction commands

```text
cargo build --release -p objekte-search-core \
  --features ic-stable-prototype --example bench_ic_stable_index

target/release/examples/bench_ic_stable_index \
  --mode source-build \
  --root "$PWD/examples/musenalm" \
  --alphabet ascii --build-workers 8 \
  --result-dir bench/results/ic-stable-index-prototype/ascii-p8

/home/simon/source/objekte-baseline-ic-stable-index-prototype/target/release/examples/bench_build_musenalm \
  --root "$PWD/examples/musenalm" \
  --alphabet ascii --build-workers 8
```

Machine-readable candidate output was written to the requested result
directories (`build.json`, `memory.json`, `components.json`, `persistence.json`,
`validation.json`, `primitives.json`, `mutations.json`,
`campaign-manifest.json`). Raw campaign output was retained outside the
repository under `/tmp/opencode/icss-campaign` for this run; it is reproducible
with the commands above and is not committed.

## Remaining limitations and deviations

1. **Construction source:** the benchmark currently builds the native
   `Repository` first and then transfers completed native indexes into
   independent stable components. It does not yet run a direct
   batch-contribution-to-stable-only build. This is labelled
   `native_build_then_independent_component_transfer` in `build.json`. The
   component-transfer stage alone is already 1.83x native, so this deviation
   does not change the performance-rejection decision.
2. **Composite sorts:** configured composite sorts are rejected explicitly.
   The Musenalm profile used here has none.
3. **Mutation reclamation:** file deletion is logical live-slot retirement, not
   physical posting reclamation. Dead slots and old posting bytes remain until
   a future compaction design.
4. **Concurrency:** all mutation and snapshots use exclusive prototype access.
   No live MVCC or concurrent readers during mutation are provided.
5. **Platforms:** Linux only was executed. Windows and macOS were not available
   for compile/smoke coverage; path keys are platform tagged and reject foreign
   platform images.
6. **Extended campaign:** because construction is clearly outside the 2x
   stopping threshold, the extended Unicode/query-family performance campaign
   was skipped as permitted. The required small semantic, persistence and
   mutation cases were still implemented and tested.

## Reuse

See [reuse-manifest.md](reuse-manifest.md). Donor database mechanisms were not
copied; native master structures were reused directly as semantic and
construction sources.
