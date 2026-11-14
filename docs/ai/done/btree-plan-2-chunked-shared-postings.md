# Plan 2 of 5: chunked shared postings

- Task identifier: `chunked-shared-postings`
- Status: specification. Do not start until the user assigns this task.
- Depends on: plan 1 (`posting-map-interface`) merged into `master`.
- Audited base: `master` at `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9` plus plan 1. Line numbers below refer to `c54486f893`; plan 1 shifts them slightly, so search for the quoted identifier.
- Next plan: `appendable-shards` (plan 3).

## 0. Rules for the implementing agent

1. Read `AGENTS.md` and `docs/ai/README.md` first and follow that workflow: branch `integration/chunked-shared-postings`, worktree `../objekte-integration-chunked-shared-postings`, memory `docs/ai/memory/chunked-shared-postings/`.
2. This plan adds no dependency. It uses `imbl`, which plan 1 added.
3. Work through the checkpoints in order. Each ends with a commit in which the workspace builds and all tests pass with default features.
4. Never make a test pass by deleting an assertion, loosening an expected value, adding `#[ignore]`, or comparing the implementation with itself. Section 3, D8 names the one narrow exception.
5. Do not add lint suppressions. Delete methods that end up unused.
6. Formatting: `rustfmt` and `python3 scripts/format-rust-spacing.py` on files you create or replace wholesale; only the changed lines in other files. Never `cargo fmt --all`.
7. Stop and write a blocker if a step needs a design decision that section 3 does not make.

## 1. Outcome

1. The `Shared` backing of `PostingMap<K>` stores one entry per `(key, chunk)`. A chunk is a fixed range of record slots. A write to a posting copies one chunk, not the whole posting.
2. A chunk covers 65,536 slots in normal builds and 256 slots when the cargo feature `force-shared-backing` is on. The small size makes ordinary test shards span several chunks, so the whole test suite exercises multi-chunk code.
3. `PostingIndex::freeze_shared` freezes `values` and `lengths` as well as the structural facts.
4. `BigramIndex.normal` and `.skip` have the new type `GramMap`, with a `Building` backing (today's `HashMap`) and a `Shared` backing (a frozen `PostingMap<u64>`). `BigramIndex::freeze_shared` exists.
5. `IndexedCollectionSegment::freeze_if_large` freezes every posting and gram family of the shard.
6. With default features nothing observable changes: same results, same index signature, same performance. With `force-shared-backing` the full workspace test suite and the Musenalm acceptance run pass.
7. A manual measurement test reports what the `Shared` backing costs on the real fixture. Plan 3 uses those numbers.

### Non-goals

- No change to which shards exist, how they are merged, or when they are compacted. The 2,048-record partition limit stays.
- No change to sort columns, record tables, file tables, `presence`/`nulls`/`empty_selections` bitmaps, or relations. Those are plan 3.
- No disk format. Plan 4 reuses the `(key, chunk)` layout defined here.
- No call-site changes outside `posting_map.rs`, `posting.rs`, `bigram.rs`, the new `gram_map.rs`, and one function in `mutation_segment.rs`. Plan 1 made the interface stable for exactly this reason.

## 2. Background

### 2.1 Why chunks

- A `RoaringBitmap` clone copies the whole bitmap. A common value or gram in a 150,000-record shard has a bitmap of up to about 19 KiB; in a 10-million-record shard, over 1 MiB. Copy-on-write of whole postings would copy that on every update of that key.
- Restricted to one 65,536-slot range, a bitmap is exactly one roaring container: at most 8 KiB, often a few bytes.
- Plan 4 stores each `(key, chunk)` entry as one B-tree value on disk. The layout in RAM and on disk is then the same, and a small update rewrites a few small values.
- `ActiveRecords` (`crates/core/src/inventory.rs:326`, `ACTIVE_SLOT_CHUNK = 1 << 16` at `:321`) already partitions the liveness bitmap the same way.

### 2.2 Layout

```
Building:  key                 -> RoaringBitmap (all slots)
Shared:    (key, slot >> BITS) -> Arc<RoaringBitmap> (only the slots of that range, absolute values)
```

Slots keep their absolute value inside a chunk. The complete posting of a key is the union of its chunks, and a key with a single chunk can be returned by reference without copying.

Invariants of the `Shared` backing (already enforced by the Appendix A code and its tests):

1. Every stored key has at least one entry.
2. An entry's bitmap is non-empty, with one exception: a key whose whole posting is empty has the single entry `(key, 0)` with an empty bitmap. This keeps the key countable, exactly as an empty bitmap stays in a `BTreeMap`.
3. The bitmap of `(key, chunk)` only holds slots of that chunk.
4. A counter holds the number of distinct keys.

### 2.3 What plan 1 left in place

- `crates/core/src/posting_map.rs` with `PostingMap<K>`, `SharedPostings<K>` (unchunked), `PostingIter`, `shared_backing_wanted`, `SHARED_BACKING_MIN_RECORDS`.
- `PostingIndex::freeze_shared` (structural facts only) and `IndexedCollectionSegment::freeze_if_large`, called at the three shard-sealing sites in `mutation_segment.rs`.
- `BigramIndex` with private `normal`/`skip` fields of type `HashMap<u64, RoaringBitmap>` and these methods: `gram_posting`, `gram_key_count`, `gram_posting_bytes`, `for_each_normal`, `for_each_skip`, `normal_entries`, `skip_entries`, `insert_gram_slot`, `set_gram`, `append_shifted_gram`.

### 2.4 Rough costs measured while preparing this plan

A scratch program (not this repository; 300,000 keys with tiny bitmaps, one thread, release build) gave these orders of magnitude. They are expectations, not gates.

| Operation | `BTreeMap` | `imbl::OrdMap` |
| --- | --- | --- |
| 300,000 inserts | 134 ms | 199 ms |
| 300,000 point lookups | 107 ms | 183 ms |
| Convert a built `BTreeMap` (freeze) | n/a | 171 ms |
| 1,000 copy-on-write updates on a shared 300,000-key map | n/a | 1.9 ms |

The `Shared` backing is slower per operation. That is why small shards and the static base stay on `Building`, and why this plan changes nothing observable with default features.

## 3. Decisions already made (do not reopen)

**D1. Replace the module, keep the interface.** `crates/core/src/posting_map.rs` is replaced with Appendix A. Every public name and signature of `PostingMap` is identical to plan 1, plus `chunk_of`, `split_chunks`, `ChunkId`, `POSTING_CHUNK_BITS` and `SharedPostings::chunk_count`. Appendix A and the tests in Appendix B were compiled and run inside this crate with both chunk sizes before this plan was written.

**D2. Chunk size.** `POSTING_CHUNK_BITS` is 16 without the feature and 8 with `force-shared-backing`. `ChunkId` is `u32`. Do not tune these.

**D3. Gram maps wrap `PostingMap<u64>`.** `GramMap` (Appendix C) is a new private module `crates/core/src/gram_map.rs`. Its `Shared` variant holds a frozen `PostingMap<u64>`; no second chunking implementation exists.

**D4. Freezing is idempotent and content-preserving.** `freeze()` on an already shared map does nothing. A frozen map compares equal (`==`, `to_btree_map()`) to the map it was made from.

**D5. One freeze entry point per type.**

```rust
// posting.rs, PostingIndex (replaces the plan 1 body)
pub fn freeze_shared(&mut self) {
    if let Some(facts) = &mut self.structural {
        facts.freeze();
    }
    self.values.freeze();
    self.lengths.freeze();
}

// bigram.rs, BigramIndex
/// Move both gram maps to the persistent backing.
pub fn freeze_shared(&mut self) {
    self.normal.freeze();
    self.skip.freeze();
}
```

**D6. `freeze_if_large` covers all posting and gram families.** After this plan its body is:

```rust
pub(crate) fn freeze_if_large(&mut self) {
    if !crate::posting_map::shared_backing_wanted(self.records.len()) {
        return;
    }
    for index in self.postings.fields.values_mut() {
        index.freeze_shared();
    }
    for custom in self.custom_postings.values_mut() {
        custom.index.freeze_shared();
    }
    if let Some(inference) = &mut self.inference {
        for index in inference.indexes.values_mut() {
            index.freeze_shared();
        }
    }
    self.raw.freeze_shared();
    self.fallback_text.freeze_shared();
    for index in self.custom_text.values_mut() {
        index.freeze_shared();
    }
}
```

**D7. Byte estimates.** For the `Building` backing every estimate stays exactly as it is. For the `Shared` backing an estimate is the sum over stored chunks, which differs slightly from the size of the whole bitmap. `BigramIndex::estimated_heap_bytes` becomes `self.normal.serialized_bytes() + self.skip.serialized_bytes() + 48 * (self.normal.len() + self.skip.len())`, which equals the old value for `Building`.

**D8. The one allowed test adjustment.** If a test fails only with `force-shared-backing`, and only because it asserts an exact byte estimate (`retained_bytes_estimated`, `index_family_bytes`, `estimated_heap_bytes`, `estimated_index_bytes`, `index_family_breakdown`), wrap that single assertion in `if !cfg!(feature = "force-shared-backing") { .. }`. List every such assertion in the report. Any other failure with the feature on is a real defect in the code: fix the code, not the test.

**D9. The measurement test is manual.** It is `#[ignore]`d like the existing real-fixture measurements (`build.test.rs:14`, `measure_direct_shard_build_from_real_worker_batches`) and is not a gate.

## 4. Checkpoints

### C0. Baseline

Record `START`. Run and save under `docs/ai/memory/chunked-shared-postings/start/`:

```sh
cargo test --workspace --locked --no-fail-fast
cargo test --workspace --locked --no-fail-fast --features objekte-search-core/force-shared-backing
cargo clippy --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
cargo run --release --example accept_musenalm -- --alphabet ascii
cargo run --release --example accept_musenalm -- --alphabet unicode
python3 bench/run_build_regressions.py --result-dir /tmp/objekte-chunked-shared-postings/build-start
python3 bench/run_query_regressions.py quick --comparison repair \
  --baseline bench/results/musenalm/controlled-foundation-performance-92cdb92b19 \
  --result-dir /tmp/objekte-chunked-shared-postings/query-start
```

Write down test counts per crate for both test runs, the Clippy warning count, and the two index signatures from `build-start/gate.json`.

### C1. Chunked `SharedPostings`

1. Replace `crates/core/src/posting_map.rs` with Appendix A.
2. Append the three tests of Appendix B to the end of `crates/core/src/posting_map.test.rs`. Keep the four existing tests unchanged.
3. Run:

```sh
cargo test -p objekte-search-core --locked --lib posting_map
cargo test -p objekte-search-core --locked --lib posting_map --features force-shared-backing
cargo test --workspace --locked --no-fail-fast
cargo test --workspace --locked --no-fail-fast --features objekte-search-core/force-shared-backing
```

   Expected: 7 `posting_map` tests pass in each of the first two runs. The two workspace runs pass with the C0 counts plus 3. In the forced run every sealed shard now holds structural facts in 256-slot chunks.

Commit: `Store shared postings in fixed slot chunks`.

### C2. Freeze values and lengths

1. Change `PostingIndex::freeze_shared` to the D5 body.
2. Add `#[cfg(test)] pub(crate) fn postings_are_shared(&self) -> bool` to `PostingIndex`, returning `self.values.is_shared() && self.lengths.is_shared()`.
3. Add this test to `posting.test.rs`:

   `frozen_posting_index_reads_like_native`. Build one `PostingIndex` for a multi-valued string field and one for a number field with a deterministic loop (no random crate): about 300 records at slots spread over three chunk ranges, for example `slot = i * 700` for `i in 0..300`, with about 40 distinct values so that several values span more than one chunk. Call `finish_stats()`. Clone each index and call `freeze_shared()` on the clone. Assert, for the native index and the frozen clone:
   - `value_entries()` and `length_entries()` are equal;
   - `distinct_values()`, `distinct_lengths()`, `stats.min_key`, `stats.max_key`, `stats.distinct_count` are equal (call `finish_stats()` on the clone first);
   - for every key present and for two keys absent: `value_cardinality(key)` equal, and `value_posting(key).map(Cow::into_owned)` equal;
   - for at least five `(lower, upper)` bound pairs and both directions: collecting `value_range(lower, upper, descending)` into `Vec<(ScalarKey, RoaringBitmap)>` gives equal vectors;
   - for every operator in `Gt, Ge, Lt, Le` and three probe keys: `union_scalar_range(key, operator, None)` equal, and equal again with `Some(&restriction)` for a restriction bitmap;
   - `union_id_postings` equal for an `Id`-typed index with a set of 3 ids (sparse path) and a set covering more than one eighth of the keys (scan path);
   - a clone of the frozen index taken before `insert_value_slot` is unchanged after the insert.

4. In `mutation_segment.test.rs`, extend `sealed_shard_backing_follows_policy` (added by plan 1): also assert `postings_are_shared() == cfg!(feature = "force-shared-backing")` for every posting index of the shard.
5. Run the two workspace test commands. With the feature on, shard queries now read values and lengths from 256-slot chunks.

Commit: `Freeze value and length postings with the shard`.

### C3. `GramMap`

1. Create `crates/core/src/gram_map.rs` from Appendix C and `crates/core/src/gram_map.test.rs` from Appendix D. Add `mod gram_map;` to `lib.rs` after `mod posting_map;`.
2. In `bigram.rs` change the two fields of `BigramIndex` to `normal: GramMap` and `skip: GramMap`, and convert the methods:

| Method (`bigram.rs`) | New body |
| --- | --- |
| `new` (`:288`) | `normal: GramMap::default(), skip: GramMap::default()` |
| `into_index_in_pool` (`:806`), test-only `into_index` (`:828`) | `normal: GramMap::from_hash_map(normal?)`, same for `skip` |
| `for_each_normal`, `for_each_skip` | `self.normal.for_each_sorted(visit)` / `self.skip.for_each_sorted(visit)` |
| `add_record` (`:321`) | `self.normal.insert_slot(gram, slot.0);` / `self.skip.insert_slot(gram, slot.0);` |
| test-only `merge_shifted` (`:364`) | iterate `batch.normal.to_btree_map()` and call `self.normal.append_shifted(key, &posting, base)?`; same for skip |
| `candidates_prepared` (`:387`) | see below |
| `estimate_prepared` (`:419`) | `if skip { self.skip.cardinality(key) } else { self.normal.cardinality(key) }` inside the existing `.map(..).min()` |
| `validate` (`:435`) | `!self.normal.all_bitmaps(in_bounds) \|\| !self.skip.all_bitmaps(in_bounds)` where `in_bounds = \|column: &RoaringBitmap\| column.max().is_none_or(\|slot\| slot < self.record_count)` |
| `validate_domain` (`:272`) | `!self.normal.all_bitmaps(\|slots\| slots.is_subset(allowed)) \|\| !self.skip.all_bitmaps(\|slots\| slots.is_subset(allowed))` |
| `estimated_heap_bytes` (`:450`) | D7 |
| `gram_posting` | `if skip { self.skip.get(key) } else { self.normal.get(key) }` |
| `gram_key_count` | `self.normal.len() + self.skip.len()` |
| `gram_posting_bytes` | `self.normal.serialized_bytes() + self.skip.serialized_bytes()` |
| `normal_entries`, `skip_entries` | `self.normal.to_btree_map()` / `self.skip.to_btree_map()` |
| `insert_gram_slot`, `set_gram`, `append_shifted_gram` | forward to `insert_slot`, `replace`, `append_shifted` of the selected map |

   `candidates_prepared` keeps its logic; only the column type changes from `&RoaringBitmap` to `Cow<RoaringBitmap>`:

```rust
pub(crate) fn candidates_prepared(&self, request: &BigramRequest) -> Option<RoaringBitmap> {
    let grams = request.grams(self.alphabet);
    if grams.is_empty() {
        return None;
    }
    let mut columns = Vec::new();
    for &(skip, key) in grams {
        let column = if skip {
            self.skip.get(key)
        } else {
            self.normal.get(key)
        };
        let Some(column) = column else {
            return Some(RoaringBitmap::new());
        };
        columns.push(column);
    }
    columns.sort_by_key(|column| column.len());
    let mut columns = columns.into_iter();
    let mut result = columns.next()?.into_owned();
    for column in columns {
        result &= column.as_ref();
        if result.is_empty() {
            break;
        }
    }
    Some(result)
}
```

3. Add `BigramIndex::freeze_shared` (D5) and `#[cfg(test)] pub(crate) fn grams_are_shared(&self) -> bool`.
4. Change `freeze_if_large` to the D6 body.
5. Add to `bigram.test.rs`:

   `frozen_bigram_index_matches_native`. Build a `BigramIndex` with `add_record` over at least 600 short texts so that record slots pass 256 and, with a few records added at high slots through `insert_gram_slot`, also pass 65,536. Clone and `freeze_shared()` the clone. Assert `normal_entries()` and `skip_entries()` equal; `gram_key_count()` equal; for ten literals (present, absent, shorter than a gram, with non-ASCII characters) `candidates(literal)` and `estimate_min_cardinality(literal)` equal; `validate(record_count)` gives the same result; a clone taken before `insert_gram_slot` on the frozen index is unchanged afterwards.

6. Extend `sealed_shard_backing_follows_policy`: assert `raw.grams_are_shared()` and `fallback_text.grams_are_shared()` equal `cfg!(feature = "force-shared-backing")`.
7. Run the two workspace test commands and both acceptance runs with the feature:

```sh
cargo run --release -p objekte-search-core --features force-shared-backing --example accept_musenalm -- --alphabet ascii
cargo run --release -p objekte-search-core --features force-shared-backing --example accept_musenalm -- --alphabet unicode
```

8. Apply D8 where it applies and nowhere else.

Commit: `Give bigram maps a persistent backing`.

### C4. Measure the shared backing on the real fixture

Add to `mutation_segment.test.rs`:

```rust
#[test]
#[ignore = "manual real-fixture shared-backing measurement; run in release with --nocapture"]
fn measure_shared_backing_on_real_fixture() { .. }
```

Follow the pattern of `measure_direct_shard_build_from_real_worker_batches` (`build.test.rs:14`): read the fixture root from an environment variable, here `OBJEKTE_SHARED_BACKING_FIXTURE`, load it with `crate::repository_configuration::load_repository_configuration`, and open it with `Repository::open_with_options`. Steps:

1. Open the Musenalm fixture as a static `Repository` with default options and take the `contents` collection (133,948 records).
2. Convert it with `crate::lifecycle_compaction::base_as_segment(collection, repository.files())`. Call this shard `native`.
3. `IndexedCollectionSegment` is not `Clone`, so call `base_as_segment` a second time to get `shared`. Time a call that freezes every family of `shared` regardless of the policy: move the loops of `freeze_if_large` into a new `pub(crate) fn freeze_all(&mut self)` and make `freeze_if_large` call it after the policy check.
4. Print one line per measurement, all times as medians of 5 repetitions:

| Label | What to measure |
| --- | --- |
| `freeze_ms` | time of `freeze_all` on a freshly converted shard |
| `keys` / `chunks` | total distinct keys and total stored chunks, summed over posting fields; and separately over gram maps. Add `#[cfg(test)]` counting helpers as needed. |
| `estimate_native_bytes` / `estimate_shared_bytes` | `accounting.index_family_bytes` after `recompute_accounting()` on each |
| `lookup_native_ns` / `lookup_shared_ns` | 10,000 `value_posting` calls on the posting field with the most distinct values, keys taken evenly from `value_entries()` |
| `scan_native_ms` / `scan_shared_ms` | one full `for_each_value` over that field, summing `len()` |
| `fts_native_us` / `fts_shared_us` | `raw.candidates(literal)` for 100 literals of 5 to 10 characters taken from record ids or values of the fixture |
| `clone_shared_ns` | `shared.clone()` of the whole shard's posting and gram families (clone the fields, not the dense tables) |
| `cow_value_us` | on a clone of the frozen field: 1,000 `insert_value_slot(existing_key, new_slot)` calls with distinct existing keys and `new_slot = record_count + i`; total time / 1,000 |
| `cow_gram_us` | on a clone of the frozen `raw` index: 1,000 `insert_gram_slot(false, existing_key, new_slot)` calls; total time / 1,000 |

5. Run it:

```sh
OBJEKTE_SHARED_BACKING_FIXTURE="$PWD/examples/musenalm" \
  cargo test --release -p objekte-search-core --locked --lib measure_shared_backing_on_real_fixture -- --ignored --nocapture
```

6. Save the output as `docs/ai/generated/chunked-shared-postings-measurement.md` with the command, the commit and the machine description. Plan 3 reads this file.

Commit: `Measure the shared posting backing on Musenalm`.

### C5. Validate and report

1. Re-run every C0 command into `*-final` directories; pass `--baseline /tmp/objekte-chunked-shared-postings/build-start/gate.json` to the build gate.
2. Check the gates in section 5.
3. Update `docs/development/posting-map.md`: describe the chunk layout (section 2.2 of this plan), the two chunk sizes, and `GramMap`.
4. Complete the task per `docs/ai/README.md` section 7.

## 5. Gates

| Gate | Requirement |
| --- | --- |
| G1 | `cargo test --workspace --locked --no-fail-fast` passes; counts are C0 plus the new tests (3 `posting_map`, 1 `gram_map`, 1 `posting`, 1 `bigram`, 1 ignored measurement). |
| G2 | The same with `--features objekte-search-core/force-shared-backing` passes with the same counts. Every D8 adjustment is listed. |
| G3 | Index signatures (ASCII, Unicode) equal C0; the build gate reports no failure. |
| G4 | `accept_musenalm` passes for ASCII and Unicode with default features and with the feature. |
| G5 | Clippy warning count not higher than C0. |
| G6 | Default-feature build gate: median time and peak RSS within 5% of C0. |
| G7 | Default-feature quick query gate passes. |

G6 and G7 follow the same reporting rule as plan 1: always report; investigate and explain 5% to 15%; block above 15%.

There is no performance gate for the forced feature. Its purpose is correctness coverage.

## 6. Pitfalls

- **The forced feature changes the chunk size.** A test that hard-codes `65_536` or `1 << 16` for chunk boundaries is wrong; use `1 << POSTING_CHUNK_BITS`.
- **Never store relative slots.** Chunks hold absolute slot numbers. `split_chunks` and `append_shifted` already do this.
- **Do not add a second chunk implementation for grams.** `GramMap::Shared` wraps `PostingMap<u64>`.
- **`get` may allocate.** For a key spanning several chunks, `get` builds the union. Do not call `get` in a loop when `cardinality` answers the question.
- **Hash-map iteration order is random.** `GramMap::for_each_sorted` sorts keys for the `Building` backing; never iterate the inner `HashMap` directly where order matters (signatures, tests).
- **Production code freezes only through `freeze_if_large`.** `freeze_all` is called by `freeze_if_large` and by tests, nowhere else.
- **Static base stays native.** Do not freeze anything in `Repository`/`CollectionState`.

## 7. The completion report must include

- `START`, branch, commits per checkpoint.
- C0 and final values: test counts (default and forced), Clippy count, index signatures, build gate medians, quick query gate result.
- The list of D8 adjustments (file, test, assertion), or "none".
- The measurement table from C4.
- Deviations and their explanation.

## Appendix A. `crates/core/src/posting_map.rs` (replaces the plan 1 file)

Copy exactly. Then apply the repository spacing script to this file.

```rust
//! Ordered key -> record-slot postings with two interchangeable backings.
//!
//! `Building` is the native `BTreeMap` used while an index is constructed and
//! by small immutable shards. `Shared` is a persistent (copy-on-write) ordered
//! map whose values are split into fixed slot ranges ("chunks"), so that a
//! clone is cheap and a write copies only one small chunk.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;

use imbl::OrdMap;
use roaring::RoaringBitmap;

/// High bits of a record slot; names one fixed slot range of a posting.
pub type ChunkId = u32;

/// Production chunks hold 65,536 slots, one roaring container. The forced
/// validation feature uses 256 so small test shards span several chunks.
#[cfg(not(feature = "force-shared-backing"))]
pub const POSTING_CHUNK_BITS: u32 = 16;
#[cfg(feature = "force-shared-backing")]
pub const POSTING_CHUNK_BITS: u32 = 8;

pub fn chunk_of(slot: u32) -> ChunkId {
    slot >> POSTING_CHUNK_BITS
}

fn chunk_mask(chunk: ChunkId) -> RoaringBitmap {
    let start = chunk << POSTING_CHUNK_BITS;
    let end = start | ((1u32 << POSTING_CHUNK_BITS) - 1);
    let mut mask = RoaringBitmap::new();
    mask.insert_range(start..=end);
    mask
}

/// Split one posting into per-chunk bitmaps, ascending by chunk. Every part
/// is non-empty. Slots keep their absolute values.
pub fn split_chunks(posting: RoaringBitmap) -> Vec<(ChunkId, RoaringBitmap)> {
    let (Some(min), Some(max)) = (posting.min(), posting.max()) else {
        return Vec::new();
    };
    let (first, last) = (chunk_of(min), chunk_of(max));
    if first == last {
        return vec![(first, posting)];
    }
    let span = u64::from(last - first) + 1;
    let mut parts = Vec::new();
    if posting.len() >= span.saturating_mul(4) {
        // Dense: one container-level intersection per chunk.
        for chunk in first..=last {
            let part = &posting & &chunk_mask(chunk);
            if !part.is_empty() {
                parts.push((chunk, part));
            }
        }
    } else {
        // Sparse: one pass over the members.
        let mut current: Option<(ChunkId, RoaringBitmap)> = None;
        for slot in posting.iter() {
            let chunk = chunk_of(slot);
            match &mut current {
                Some((open, part)) if *open == chunk => {
                    part.push(slot);
                }
                _ => {
                    if let Some(done) = current.take() {
                        parts.push(done);
                    }
                    let mut part = RoaringBitmap::new();
                    part.push(slot);
                    current = Some((chunk, part));
                }
            }
        }
        if let Some(done) = current {
            parts.push(done);
        }
    }
    parts
}

/// Persistent chunked postings. Invariants:
/// 1. every stored key has at least one entry;
/// 2. an entry's bitmap is non-empty, except the single entry `(key, 0)` of a
///    key whose whole posting is empty;
/// 3. the bitmap of `(key, chunk)` only holds slots of that chunk;
/// 4. `keys` is the number of distinct keys.
#[derive(Clone, Debug)]
pub struct SharedPostings<K: Ord + Clone> {
    chunks: OrdMap<(K, ChunkId), Arc<RoaringBitmap>>,
    keys: usize,
}

type ChunkEntry<'a, K> = (&'a (K, ChunkId), &'a Arc<RoaringBitmap>);

impl<K: Ord + Clone> Default for SharedPostings<K> {
    fn default() -> Self {
        Self {
            chunks: OrdMap::new(),
            keys: 0,
        }
    }
}

impl<K: Ord + Clone> SharedPostings<K> {
    fn key_bounds(key: &K) -> (Bound<(K, ChunkId)>, Bound<(K, ChunkId)>) {
        (
            Bound::Included((key.clone(), 0)),
            Bound::Included((key.clone(), ChunkId::MAX)),
        )
    }

    pub fn from_building(source: BTreeMap<K, RoaringBitmap>) -> Self {
        let mut keys = 0usize;
        let mut entries = Vec::new();
        for (key, posting) in source {
            keys += 1;
            let parts = split_chunks(posting);
            if parts.is_empty() {
                entries.push(((key, 0), Arc::new(RoaringBitmap::new())));
                continue;
            }
            for (chunk, part) in parts {
                entries.push(((key.clone(), chunk), Arc::new(part)));
            }
        }
        Self {
            chunks: entries.into_iter().collect(),
            keys,
        }
    }

    pub fn len(&self) -> usize {
        self.keys
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.chunks.range(Self::key_bounds(key)).next().is_some()
    }

    pub fn get(&self, key: &K) -> Option<Cow<'_, RoaringBitmap>> {
        let mut entries = self.chunks.range(Self::key_bounds(key));
        let (_, first) = entries.next()?;
        let Some((_, second)) = entries.next() else {
            return Some(Cow::Borrowed(first.as_ref()));
        };
        let mut whole = first.as_ref().clone();
        whole |= second.as_ref();
        for (_, more) in entries {
            whole |= more.as_ref();
        }
        Some(Cow::Owned(whole))
    }

    pub fn cardinality(&self, key: &K) -> u64 {
        self.chunks
            .range(Self::key_bounds(key))
            .map(|(_, part)| part.len())
            .sum()
    }

    fn max_slot(&self, key: &K) -> Option<u32> {
        self.chunks
            .range(Self::key_bounds(key))
            .next_back()
            .and_then(|(_, part)| part.max())
    }

    /// Add `part` (already restricted to `chunk`) to `key`.
    fn merge_part(&mut self, key: &K, chunk: ChunkId, part: RoaringBitmap) {
        let address = (key.clone(), chunk);
        if let Some(existing) = self.chunks.get_mut(&address) {
            *Arc::make_mut(existing) |= part;
            return;
        }
        if !self.contains_key(key) {
            self.keys += 1;
        } else if chunk != 0
            && self
                .chunks
                .get(&(key.clone(), 0))
                .is_some_and(|zero| zero.is_empty())
        {
            // Drop the empty-posting placeholder now that the key has members.
            self.chunks.remove(&(key.clone(), 0));
        }
        self.chunks.insert(address, Arc::new(part));
    }

    fn ensure_key(&mut self, key: &K) {
        if !self.contains_key(key) {
            self.keys += 1;
            self.chunks
                .insert((key.clone(), 0), Arc::new(RoaringBitmap::new()));
        }
    }

    pub fn insert_slot(&mut self, key: &K, slot: u32) -> bool {
        let chunk = chunk_of(slot);
        if let Some(existing) = self.chunks.get_mut(&(key.clone(), chunk)) {
            return Arc::make_mut(existing).insert(slot);
        }
        let mut part = RoaringBitmap::new();
        part.insert(slot);
        self.merge_part(key, chunk, part);
        true
    }

    pub fn append_shifted(
        &mut self,
        key: &K,
        source: &RoaringBitmap,
        base: u32,
    ) -> Result<(), String> {
        let Some(source_max) = source.max() else {
            self.ensure_key(key);
            return Ok(());
        };
        base.checked_add(source_max).ok_or("record slot overflow")?;
        let first_new = base + source.min().unwrap_or(0);
        if self.max_slot(key).is_some_and(|existing| first_new <= existing) {
            return Err("batch bitmap slots overlap or are out of order".into());
        }
        let mut current: Option<(ChunkId, RoaringBitmap)> = None;
        for slot in source.iter() {
            let shifted = base + slot;
            let chunk = chunk_of(shifted);
            match &mut current {
                Some((open, part)) if *open == chunk => {
                    part.push(shifted);
                }
                _ => {
                    if let Some((done, part)) = current.take() {
                        self.merge_part(key, done, part);
                    }
                    let mut part = RoaringBitmap::new();
                    part.push(shifted);
                    current = Some((chunk, part));
                }
            }
        }
        if let Some((done, part)) = current {
            self.merge_part(key, done, part);
        }
        Ok(())
    }

    fn remove_key(&mut self, key: &K) -> bool {
        let addresses = self
            .chunks
            .range(Self::key_bounds(key))
            .map(|(address, _)| address.clone())
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return false;
        }
        for address in addresses {
            self.chunks.remove(&address);
        }
        self.keys -= 1;
        true
    }

    pub fn replace(&mut self, key: &K, posting: RoaringBitmap) {
        self.remove_key(key);
        let parts = split_chunks(posting);
        if parts.is_empty() {
            self.ensure_key(key);
            return;
        }
        for (chunk, part) in parts {
            self.merge_part(key, chunk, part);
        }
    }

    pub fn retain_keys(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let mut doomed = Vec::new();
        let mut previous: Option<&K> = None;
        for ((key, _), _) in self.chunks.iter() {
            if previous == Some(key) {
                continue;
            }
            previous = Some(key);
            if !keep(key) {
                doomed.push(key.clone());
            }
        }
        for key in doomed {
            self.remove_key(&key);
        }
    }

    pub fn clear(&mut self) {
        self.chunks = OrdMap::new();
        self.keys = 0;
    }

    pub fn first_key(&self) -> Option<&K> {
        self.chunks.get_min().map(|(address, _)| &address.0)
    }

    pub fn last_key(&self) -> Option<&K> {
        self.chunks.get_max().map(|(address, _)| &address.0)
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn serialized_bytes(&self) -> usize {
        self.chunks
            .iter()
            .map(|(_, part)| part.serialized_size())
            .sum()
    }

    pub fn all_parts(&self, mut valid: impl FnMut(&RoaringBitmap) -> bool) -> bool {
        self.chunks.iter().all(|(_, part)| valid(part))
    }

    fn entries<'a>(
        &'a self,
        lower: Bound<&K>,
        upper: Bound<&K>,
        descending: bool,
    ) -> Box<dyn Iterator<Item = ChunkEntry<'a, K>> + 'a> {
        let lower_address = match lower {
            Bound::Included(key) => Bound::Included((key.clone(), 0)),
            Bound::Excluded(key) => Bound::Excluded((key.clone(), ChunkId::MAX)),
            Bound::Unbounded => Bound::Unbounded,
        };
        let upper_address = match upper {
            Bound::Included(key) => Bound::Included((key.clone(), ChunkId::MAX)),
            Bound::Excluded(key) => Bound::Excluded((key.clone(), 0)),
            Bound::Unbounded => Bound::Unbounded,
        };
        // An inverted range is empty here instead of a panic.
        let inverted = match (lower, upper) {
            (Bound::Included(a), Bound::Included(b)) => a > b,
            (Bound::Included(a), Bound::Excluded(b))
            | (Bound::Excluded(a), Bound::Included(b))
            | (Bound::Excluded(a), Bound::Excluded(b)) => a >= b,
            _ => false,
        };
        if inverted {
            return Box::new(std::iter::empty());
        }
        let range = self.chunks.range((lower_address, upper_address));
        if descending {
            Box::new(range.rev())
        } else {
            Box::new(range)
        }
    }
}

/// Shards with at least this many records use the shared backing.
pub const SHARED_BACKING_MIN_RECORDS: usize = 16_384;

/// Whether a shard of `record_count` records should hold its postings in the
/// shared backing. The validation feature forces it for every shard so the
/// whole test suite exercises that backing.
pub fn shared_backing_wanted(record_count: usize) -> bool {
    cfg!(feature = "force-shared-backing") || record_count >= SHARED_BACKING_MIN_RECORDS
}

/// Forward-only traversal in ascending or descending key order. Each item is
/// one key with its complete posting.
pub struct PostingIter<'a, K: Ord + Clone> {
    inner: IterInner<'a, K>,
}

enum IterInner<'a, K: Ord + Clone> {
    Building(Box<dyn Iterator<Item = (&'a K, &'a RoaringBitmap)> + 'a>),
    Shared {
        entries: Box<dyn Iterator<Item = ChunkEntry<'a, K>> + 'a>,
        ahead: Option<ChunkEntry<'a, K>>,
    },
}

impl<'a, K: Ord + Clone> Iterator for PostingIter<'a, K> {
    type Item = (&'a K, Cow<'a, RoaringBitmap>);

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            IterInner::Building(entries) => entries
                .next()
                .map(|(key, posting)| (key, Cow::Borrowed(posting))),
            IterInner::Shared { entries, ahead } => {
                let (address, first) = ahead.take().or_else(|| entries.next())?;
                let key = &address.0;
                let mut whole: Option<RoaringBitmap> = None;
                for (next_address, part) in entries.by_ref() {
                    if next_address.0 != *key {
                        *ahead = Some((next_address, part));
                        break;
                    }
                    let target = whole.get_or_insert_with(|| first.as_ref().clone());
                    *target |= part.as_ref();
                }
                Some((
                    key,
                    whole.map_or(Cow::Borrowed(first.as_ref()), Cow::Owned),
                ))
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Backing<K: Ord + Clone> {
    Building(BTreeMap<K, RoaringBitmap>),
    Shared(SharedPostings<K>),
}

/// One ordered posting dictionary. All index code reads and writes postings
/// through this type; nothing else may depend on the backing.
#[derive(Clone, Debug)]
pub struct PostingMap<K: Ord + Clone> {
    backing: Backing<K>,
}

impl<K: Ord + Clone> Default for PostingMap<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Ord + Clone> PartialEq for PostingMap<K> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .zip(other.iter())
                .all(|((a, left), (b, right))| a == b && left.as_ref() == right.as_ref())
    }
}

impl<K: Ord + Clone> PostingMap<K> {
    pub fn new() -> Self {
        Self {
            backing: Backing::Building(BTreeMap::new()),
        }
    }

    pub fn from_btree_map(entries: BTreeMap<K, RoaringBitmap>) -> Self {
        Self {
            backing: Backing::Building(entries),
        }
    }

    pub fn is_shared(&self) -> bool {
        matches!(self.backing, Backing::Shared(_))
    }

    /// Switch to the persistent backing. Idempotent; contents are unchanged.
    pub fn freeze(&mut self) {
        if let Backing::Building(entries) = &mut self.backing {
            let entries = std::mem::take(entries);
            self.backing = Backing::Shared(SharedPostings::from_building(entries));
        }
    }

    pub fn len(&self) -> usize {
        match &self.backing {
            Backing::Building(entries) => entries.len(),
            Backing::Shared(shared) => shared.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains_key(&self, key: &K) -> bool {
        match &self.backing {
            Backing::Building(entries) => entries.contains_key(key),
            Backing::Shared(shared) => shared.contains_key(key),
        }
    }

    pub fn get(&self, key: &K) -> Option<Cow<'_, RoaringBitmap>> {
        match &self.backing {
            Backing::Building(entries) => entries.get(key).map(Cow::Borrowed),
            Backing::Shared(shared) => shared.get(key),
        }
    }

    pub fn cardinality(&self, key: &K) -> u64 {
        match &self.backing {
            Backing::Building(entries) => entries.get(key).map_or(0, RoaringBitmap::len),
            Backing::Shared(shared) => shared.cardinality(key),
        }
    }

    pub fn first_key(&self) -> Option<&K> {
        match &self.backing {
            Backing::Building(entries) => entries.first_key_value().map(|(key, _)| key),
            Backing::Shared(shared) => shared.first_key(),
        }
    }

    pub fn last_key(&self) -> Option<&K> {
        match &self.backing {
            Backing::Building(entries) => entries.last_key_value().map(|(key, _)| key),
            Backing::Shared(shared) => shared.last_key(),
        }
    }

    /// `entry(key).or_default().insert(slot)`.
    pub fn insert_slot(&mut self, key: K, slot: u32) -> bool {
        match &mut self.backing {
            Backing::Building(entries) => entries.entry(key).or_default().insert(slot),
            Backing::Shared(shared) => shared.insert_slot(&key, slot),
        }
    }

    /// `insert(key, posting)`: the posting replaces any previous one.
    pub fn replace(&mut self, key: K, posting: RoaringBitmap) {
        match &mut self.backing {
            Backing::Building(entries) => {
                entries.insert(key, posting);
            }
            Backing::Shared(shared) => shared.replace(&key, posting),
        }
    }

    /// `append_shifted_bitmap(entry(key).or_default(), source, base)`.
    pub fn append_shifted(
        &mut self,
        key: K,
        source: &RoaringBitmap,
        base: u32,
    ) -> Result<(), String> {
        match &mut self.backing {
            Backing::Building(entries) => {
                if let Some(max) = source.max() {
                    base.checked_add(max).ok_or("record slot overflow")?;
                }
                entries
                    .entry(key)
                    .or_default()
                    .append(source.iter().map(|slot| base + slot))
                    .map_err(|_| "batch bitmap slots overlap or are out of order")?;
                Ok(())
            }
            Backing::Shared(shared) => shared.append_shifted(&key, source, base),
        }
    }

    pub fn retain_keys(&mut self, mut keep: impl FnMut(&K) -> bool) {
        match &mut self.backing {
            Backing::Building(entries) => entries.retain(|key, _| keep(key)),
            Backing::Shared(shared) => shared.retain_keys(keep),
        }
    }

    /// Remove every key but keep the current backing.
    pub fn clear(&mut self) {
        match &mut self.backing {
            Backing::Building(entries) => entries.clear(),
            Backing::Shared(shared) => shared.clear(),
        }
    }

    pub fn iter(&self) -> PostingIter<'_, K> {
        self.range(Bound::Unbounded, Bound::Unbounded, false)
    }

    pub fn range<'a>(
        &'a self,
        lower: Bound<&K>,
        upper: Bound<&K>,
        descending: bool,
    ) -> PostingIter<'a, K> {
        let inner = match &self.backing {
            Backing::Building(entries) => {
                let range = entries.range::<K, _>((lower, upper));
                IterInner::Building(if descending {
                    Box::new(range.rev())
                } else {
                    Box::new(range)
                })
            }
            Backing::Shared(shared) => IterInner::Shared {
                entries: shared.entries(lower, upper, descending),
                ahead: None,
            },
        };
        PostingIter { inner }
    }

    pub fn to_btree_map(&self) -> BTreeMap<K, RoaringBitmap> {
        self.iter()
            .map(|(key, posting)| (key.clone(), posting.into_owned()))
            .collect()
    }

    pub fn into_entries(self) -> Vec<(K, RoaringBitmap)> {
        match self.backing {
            Backing::Building(entries) => entries.into_iter().collect(),
            shared @ Backing::Shared(_) => Self { backing: shared }
                .iter()
                .map(|(key, posting)| (key.clone(), posting.into_owned()))
                .collect(),
        }
    }

    /// Sum of the stored bitmaps' serialized sizes (a planning estimate).
    pub fn serialized_bytes(&self) -> usize {
        match &self.backing {
            Backing::Building(entries) => {
                entries.values().map(RoaringBitmap::serialized_size).sum()
            }
            Backing::Shared(shared) => shared.serialized_bytes(),
        }
    }

    /// True when `valid` holds for every stored bitmap (whole or chunk).
    pub fn all_bitmaps(&self, mut valid: impl FnMut(&RoaringBitmap) -> bool) -> bool {
        match &self.backing {
            Backing::Building(entries) => entries.values().all(|posting| valid(posting)),
            Backing::Shared(shared) => shared.all_parts(valid),
        }
    }
}

#[cfg(test)]
#[path = "posting_map.test.rs"]
mod tests;
```

## Appendix B. Tests to append to `crates/core/src/posting_map.test.rs`

```rust
/// A key whose slots span several chunks is assembled on read; a key inside
/// one chunk is borrowed. Both must equal the native posting.
#[test]
fn shared_reads_assemble_multi_chunk_postings() {
    let chunk = 1u32 << POSTING_CHUNK_BITS;
    let mut map: PostingMap<u16> = PostingMap::new();
    map.freeze();
    for slot in [1, chunk - 1, chunk, 3 * chunk + 7] {
        map.insert_slot(1, slot);
    }
    map.insert_slot(2, 5);
    assert!(matches!(map.get(&1), Some(Cow::Owned(_))));
    assert!(matches!(map.get(&2), Some(Cow::Borrowed(_))));
    assert_eq!(
        map.get(&1).unwrap().iter().collect::<Vec<_>>(),
        vec![1, chunk - 1, chunk, 3 * chunk + 7]
    );
    assert_eq!(map.cardinality(&1), 4);
    assert_eq!(map.len(), 2);
}

/// A write to a clone copies one chunk; every other chunk stays shared.
#[test]
fn shared_clone_writes_copy_one_chunk() {
    let chunk = 1u32 << POSTING_CHUNK_BITS;
    let mut entries = BTreeMap::new();
    for key in 0..50u16 {
        let posting: RoaringBitmap = [u32::from(key), chunk + 1, 2 * chunk + 2].into_iter().collect();
        entries.insert(key, posting);
    }
    let mut original = PostingMap::from_btree_map(entries);
    original.freeze();
    let mut changed = original.clone();
    changed.insert_slot(7, chunk + 9);
    let (Backing::Shared(before), Backing::Shared(after)) = (&original.backing, &changed.backing)
    else {
        panic!("both maps must use the shared backing");
    };
    assert_eq!(before.chunk_count(), 150);
    assert_eq!(after.chunk_count(), 150);
    let copied = before
        .chunks
        .iter()
        .zip(after.chunks.iter())
        .filter(|((_, left), (_, right))| !Arc::ptr_eq(left, right))
        .count();
    assert_eq!(copied, 1);
    assert_eq!(original.cardinality(&7), 3);
    assert_eq!(changed.cardinality(&7), 4);
}

/// The empty-posting placeholder keeps a key alive and disappears once the
/// key gains a member in another chunk.
#[test]
fn shared_empty_posting_keeps_its_key() {
    let chunk = 1u32 << POSTING_CHUNK_BITS;
    let mut map: PostingMap<u16> = PostingMap::new();
    map.freeze();
    map.replace(4, RoaringBitmap::new());
    assert!(map.contains_key(&4));
    assert_eq!(map.len(), 1);
    assert_eq!(map.get(&4).unwrap().len(), 0);
    map.insert_slot(4, 2 * chunk);
    assert_eq!(map.len(), 1);
    assert_eq!(map.get(&4).unwrap().iter().collect::<Vec<_>>(), vec![2 * chunk]);
    let Backing::Shared(shared) = &map.backing else {
        panic!("map must use the shared backing");
    };
    assert_eq!(shared.chunk_count(), 1);
}
```

## Appendix C. `crates/core/src/gram_map.rs`

```rust
//! Gram key -> record-slot postings for one bigram alphabet half.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use roaring::RoaringBitmap;

use crate::posting_map::PostingMap;

/// `Building` keeps the construction-time hash map. `Shared` is a frozen
/// [`PostingMap`], which gives cheap clones and chunk-sized writes.
#[derive(Clone, Debug)]
pub enum GramMap {
    Building(HashMap<u64, RoaringBitmap>),
    Shared(PostingMap<u64>),
}

impl Default for GramMap {
    fn default() -> Self {
        Self::Building(HashMap::new())
    }
}

impl GramMap {
    pub fn from_hash_map(entries: HashMap<u64, RoaringBitmap>) -> Self {
        Self::Building(entries)
    }

    pub fn is_shared(&self) -> bool {
        matches!(self, Self::Shared(_))
    }

    /// Switch to the persistent backing. Idempotent; contents are unchanged.
    pub fn freeze(&mut self) {
        if let Self::Building(entries) = self {
            let ordered = std::mem::take(entries)
                .into_iter()
                .collect::<BTreeMap<_, _>>();
            let mut shared = PostingMap::from_btree_map(ordered);
            shared.freeze();
            *self = Self::Shared(shared);
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Building(entries) => entries.len(),
            Self::Shared(shared) => shared.len(),
        }
    }

    pub fn get(&self, key: u64) -> Option<Cow<'_, RoaringBitmap>> {
        match self {
            Self::Building(entries) => entries.get(&key).map(Cow::Borrowed),
            Self::Shared(shared) => shared.get(&key),
        }
    }

    pub fn cardinality(&self, key: u64) -> u64 {
        match self {
            Self::Building(entries) => entries.get(&key).map_or(0, RoaringBitmap::len),
            Self::Shared(shared) => shared.cardinality(&key),
        }
    }

    pub fn insert_slot(&mut self, key: u64, slot: u32) -> bool {
        match self {
            Self::Building(entries) => entries.entry(key).or_default().insert(slot),
            Self::Shared(shared) => shared.insert_slot(key, slot),
        }
    }

    pub fn replace(&mut self, key: u64, posting: RoaringBitmap) {
        match self {
            Self::Building(entries) => {
                entries.insert(key, posting);
            }
            Self::Shared(shared) => shared.replace(key, posting),
        }
    }

    pub fn append_shifted(
        &mut self,
        key: u64,
        source: &RoaringBitmap,
        base: u32,
    ) -> Result<(), String> {
        match self {
            Self::Building(entries) => {
                if let Some(max) = source.max() {
                    base.checked_add(max).ok_or("record slot overflow")?;
                }
                entries
                    .entry(key)
                    .or_default()
                    .append(source.iter().map(|slot| base + slot))
                    .map_err(|_| "batch bitmap slots overlap or are out of order")?;
                Ok(())
            }
            Self::Shared(shared) => shared.append_shifted(key, source, base),
        }
    }

    /// Visit every gram in ascending key order.
    pub fn for_each_sorted(
        &self,
        mut visit: impl FnMut(&u64, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        match self {
            Self::Building(entries) => {
                let mut keys = entries.keys().collect::<Vec<_>>();
                keys.sort_unstable();
                for key in keys {
                    visit(key, &entries[key])?;
                }
            }
            Self::Shared(shared) => {
                for (key, posting) in shared.iter() {
                    visit(key, posting.as_ref())?;
                }
            }
        }
        Ok(())
    }

    pub fn serialized_bytes(&self) -> usize {
        match self {
            Self::Building(entries) => {
                entries.values().map(RoaringBitmap::serialized_size).sum()
            }
            Self::Shared(shared) => shared.serialized_bytes(),
        }
    }

    /// Number of stored bitmaps: one per key while building, one per chunk
    /// once shared. Used only by byte estimates.
    pub fn all_bitmaps(&self, mut valid: impl FnMut(&RoaringBitmap) -> bool) -> bool {
        match self {
            Self::Building(entries) => entries.values().all(|posting| valid(posting)),
            Self::Shared(shared) => shared.all_bitmaps(valid),
        }
    }

    pub fn to_btree_map(&self) -> BTreeMap<u64, RoaringBitmap> {
        match self {
            Self::Building(entries) => entries
                .iter()
                .map(|(key, posting)| (*key, posting.clone()))
                .collect(),
            Self::Shared(shared) => shared.to_btree_map(),
        }
    }
}

#[cfg(test)]
#[path = "gram_map.test.rs"]
mod tests;
```

## Appendix D. `crates/core/src/gram_map.test.rs`

```rust
use super::*;

#[test]
fn gram_map_backings_agree() {
    let mut building = GramMap::default();
    for (key, slot) in [(7u64, 1u32), (7, 70_000), (3, 5), (9, 300)] {
        building.insert_slot(key, slot);
    }
    let mut shared = building.clone();
    shared.freeze();
    assert!(shared.is_shared() && !building.is_shared());
    for map in [&mut building, &mut shared] {
        let source: RoaringBitmap = [0u32, 9].into_iter().collect();
        map.append_shifted(7, &source, 200_000).unwrap();
        map.append_shifted(11, &source, 0).unwrap();
        map.replace(3, RoaringBitmap::new());
        assert!(map.append_shifted(7, &source, 5).is_err());
    }
    assert_eq!(building.to_btree_map(), shared.to_btree_map());
    assert_eq!(building.len(), shared.len());
    assert_eq!(building.cardinality(7), 4);
    assert_eq!(shared.cardinality(7), 4);
    assert_eq!(
        building.get(7).map(Cow::into_owned),
        shared.get(7).map(Cow::into_owned)
    );
    let mut seen = (Vec::new(), Vec::new());
    building.for_each_sorted(|key, _| Ok(seen.0.push(*key))).unwrap();
    shared.for_each_sorted(|key, _| Ok(seen.1.push(*key))).unwrap();
    assert_eq!(seen.0, seen.1);
    assert_eq!(seen.0, vec![3, 7, 9, 11]);
    assert!(shared.all_bitmaps(|posting| posting.len() <= 4));
    assert!(building.serialized_bytes() > 0 && shared.serialized_bytes() > 0);
}
```

## Implementation report

- Task identifier: `chunked-shared-postings`
- Completion date: 2026-11-15
- Base commit: `5d509aecf1` (plan 1 integration tip; the plan's textual base is
  `c54486f893` plus plan 1)
- Integration branch: `integration/chunked-shared-postings`
- Integration worktree:
  `/home/simon/source/objekte-second-base-integration-chunked-shared-postings`
- Implementation commits:
  - `59e62e571e` Store shared postings in fixed slot chunks
  - `7a383b074a` Freeze value and length postings with the shard
  - `32f2dd1d29` Give bigram maps a persistent backing
  - `7f18fe1ac7` Measure the shared posting backing on Musenalm
  - final documentation/report commit (this commit)
- START: `5d509aecf1`.

### Summary

The `Shared` backing of `PostingMap<K>` now stores one entry per `(key, chunk)`
with absolute slot values. `POSTING_CHUNK_BITS` is 16 in normal builds and 8
under `force-shared-backing`; `chunk_of`, `split_chunks`, `ChunkId` and
`SharedPostings::chunk_count` implement and inspect the layout. Multi-chunk
reads assemble a `Cow::Owned` union; single-chunk reads stay borrowed. A
copy-on-write update copies one chunk.

`PostingIndex::freeze_shared` now freezes structural facts, values and lengths.
`BigramIndex` stores both gram maps in the new `GramMap` (`Building` hash map or
a frozen `PostingMap<u64>`), with sorted iteration for signatures and tests.
`IndexedCollectionSegment::freeze_if_large` freezes every posting and gram
family, and `freeze_all` is available to tests and the measurement. The manual
real-fixture measurement is committed as
`docs/ai/generated/chunked-shared-postings-measurement.md`.

### Validation

- C0 (`docs/ai/memory/chunked-shared-postings/start/`):
  default tests 828 passed / 17 ignored; forced tests 828 / 17; Clippy 237
  warning lines; ASCII/Unicode signatures
  `5116df6395bf7c92fae99422dcf7f4456852dfa6acdf275dd0915d22114cb53e` /
  `595f189a9ee32ced1ca07f17d7c61fd5b019038073e24d42df1a4c7ab9bada07`;
  build gate ASCII 1.035 s / 490.5 MB, Unicode 1.071 s / 510.6 MB.
- Final (`docs/ai/memory/chunked-shared-postings/final2/`):
  default tests 834 passed / 18 ignored, forced tests 834 / 18 (the extra
  ignored test is the manual measurement); Clippy 236 warning lines; the
  detailed-profiling check passes; `accept_musenalm` passes for ASCII and
  Unicode with default features and with `force-shared-backing`.
- Index signatures equal C0 in every run; the build gate reports no failure.
- Build gate medians over three final repetitions: ASCII 1.096 s (5.9% above
  C0) and Unicode 1.130 s (5.6% above C0), RSS +0.8% / +0.0%. The runs are
  inside the 5%-15% repeat-and-explain band; the extra branch and chunk-split
  bookkeeping of the wrapper explain the small default-feature cost, and the
  repetitions were taken while another build shared the machine.
- Quick query gate: the named foundation baseline is path-sensitive and was
  recorded at `/home/simon/source/objekte`; this checkout is
  `/home/simon/source/objekte-second-base`, so the same three compatibility
  failures occur as in C0 (`corpus_fingerprint`, missing baseline schema
  manifest, semantic registrations). The final aggregate ratio is 0.921 and
  there are no latency confirmation failures. This is an environmental
  baseline limitation, not a regression; the run is retained under
  `/tmp/objekte-chunked-shared-postings/query-final2`.

### D8 adjustments

None. No exact-byte assertion had to be disabled under
`force-shared-backing`.

### Measurement

From `docs/ai/generated/chunked-shared-postings-measurement.md` (medians of 5,
`contents` 133,948 records): freeze 57.1 ms; 140,781 posting keys / 148,687
chunks; 5,264 gram keys / 14,396 gram chunks; estimate 52.59 MB native versus
52.72 MB shared; lookup 69.7 ns native versus 193.1 ns shared; scan 2.29 ms
versus 6.21 ms; 100 FTS probes 2,247 µs versus 5,142 µs; shard family clone
75.6 µs; copy-on-write value insert 0.94 µs, gram insert 0.52 µs.

### Deviations

- Branches are stacked: plan 2 branches from plan 1's integration tip because
  agents may not merge into `master`.
- `scripts/format-rust-spacing.py` was applied through its `spaced()` function
  to the replaced `posting_map.rs` only, after `rustfmt`; it was not run over
  the whole repository because that invokes `cargo fmt --all`, which rule 7
  forbids.
- Two warnings introduced by the Appendix code were removed without changing
  behaviour: `RoaringBitmap::push` became `try_push` (slots are ascending), and
  the gram test closures now return `()` explicitly. The clippy count is below
  C0.
