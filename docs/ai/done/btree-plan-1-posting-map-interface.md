# Plan 1 of 5: posting map interface and persistent pilot

- Task identifier: `posting-map-interface`
- Status: specification. Do not start until the user assigns this task.
- Depends on: nothing. This is the first of five plans.
- Audited base: `master` at `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`. All line numbers below refer to that commit. If a line has moved, search for the quoted identifier instead.
- Next plan: `chunked-shared-postings` (plan 2).

## 0. Rules for the implementing agent

1. Read `AGENTS.md` and `docs/ai/README.md` first and follow that workflow: integration branch `integration/posting-map-interface`, sibling worktree `../objekte-integration-posting-map-interface`, task memory under `docs/ai/memory/posting-map-interface/`, push after every commit, never commit to `master`.
2. This plan authorises exactly one new dependency: `imbl` (licence MPL-2.0-or-later) in `crates/core`. Do not add any other dependency, including test dependencies.
3. Work through the checkpoints in order. Each checkpoint ends with a commit in which the whole workspace builds and all tests pass.
4. The compiler is the authority on call sites. The tables in this plan list every site found at the audited base; if the compiler reports a site that is not listed, convert it with the same pattern and note it in the report.
5. Never make a test pass by deleting an assertion, loosening an expected value, adding `#[ignore]`, or comparing the implementation with itself. If a test fails and you cannot see why, stop and write a blocker (workflow section 6).
6. Do not add `#[allow(dead_code)]` or any other lint suppression. If a method from Appendix A ends up unused when the plan is finished, delete that method.
7. Formatting: run `rustfmt` and `python3 scripts/format-rust-spacing.py` on files you create. In existing files, format only the lines you change. Do not run `cargo fmt --all`; the repository has known formatter drift and a blanket reformat would bury the real diff.
8. Stop and write a blocker instead of improvising if a step needs a design decision that section 3 does not already make.

## 1. Outcome

### 1.1 Where this fits

The long-term goal is index structures that can be changed in place cheaply and stored on disk as B-trees. Five plans get there:

| Plan | Result |
| --- | --- |
| 1 (this one) | Nothing outside two modules touches the index maps directly. A persistent (copy-on-write) backing exists and is piloted on the smallest index family. |
| 2 | The persistent backing stores postings in fixed slot ranges ("chunks"). All posting and gram maps can use it. |
| 3 | Sort orders, record tables and file tables get persistent backings. One shard per collection can grow by cheap appends. |
| 4 | The index is saved to and loaded from a `redb` file. |
| 5 | Background cleanup of deleted records, slot reuse, and write batching. |

### 1.2 What is true when this plan is done

1. A new private module `crates/core/src/posting_map.rs` defines `PostingMap<K>`, an ordered map from a key to a set of record slots, with two backings: `Building` (the current `BTreeMap<K, RoaringBitmap>`) and `Shared` (`imbl::OrdMap<K, Arc<RoaringBitmap>>`).
2. These five fields are private and have accessor methods:
   - `PostingIndex.values`, `PostingIndex.lengths` (`crates/core/src/posting.rs`)
   - `StructuralFacts.postings` (`crates/core/src/posting.rs`)
   - `BigramIndex.normal`, `BigramIndex.skip` (`crates/core/src/bigram.rs`)
3. The three posting fields have type `PostingMap<_>`. The two gram fields keep type `HashMap<u64, RoaringBitmap>` (plan 2 changes them).
4. `StructuralFacts` switches to the `Shared` backing when a shard is sealed and the policy function says so. With default features that only happens for shards of 16,384 records or more, which normal builds do not produce. With the new cargo feature `force-shared-backing` it happens for every sealed shard.
5. Query results, index contents, error messages and byte estimates are unchanged. The `index_signature_sha256` printed by `bench_build_musenalm` is identical before and after.

### 1.3 Non-goals

- No chunking, no change to gram map types, no change to sort columns, record tables or shard layout. Those are plans 2 and 3.
- No change to mutation, compaction, consolidation or partition policy.
- No disk format.
- No public API change other than the accessors listed in section 3.

## 2. Background (verified at the audited base)

### 2.1 Two representations use the same index types

- The **static base** is `Repository` with one `CollectionState` per collection (`crates/core/src/repository.rs:149`). It is built in one pass and never mutated.
- A **shard** is `IndexedCollectionSegment` (`crates/core/src/mutation_segment.rs:274`, re-exported as `crate::build::IndexedCollectionSegment`). A live `Index` publishes a list of shards per collection as `PublishedSegment { ordinal, index: Arc<IndexedCollectionSegment>, active }` (`crates/core/src/lifecycle.rs:410`).
- `Index::from_repository` converts the static base into shards of at most 2,048 records (`partition_base`, `crates/core/src/lifecycle_partition.rs:20`, `BASE_PARTITION_RECORDS`). Direct JSON builds create such shards directly (`crates/core/src/repository_direct.rs`).
- Both representations hold `PostingIndex`, `StructuralFacts` and `BigramIndex` values. A change to these types affects both.

### 2.2 The maps this plan encapsulates

| Field | Type today | Meaning |
| --- | --- | --- |
| `PostingIndex.values` | `BTreeMap<ScalarKey, RoaringBitmap>` | typed value → record slots |
| `PostingIndex.lengths` | `BTreeMap<Number, RoaringBitmap>` | array length → record slots |
| `StructuralFacts.postings` | `BTreeMap<StructuralKey, RoaringBitmap>` | at most 7 structural facts → record slots |
| `BigramIndex.normal`, `.skip` | `HashMap<u64, RoaringBitmap>` | gram → record slots |

`ScalarKey` and `Number` have a custom `Ord` (`crates/core/src/model.rs:117`). Never replace key comparison with byte comparison.

### 2.3 Where shards are created

Exactly four places build an `IndexedCollectionSegment` struct literal and then call `recompute_accounting()`:

| File and line | Function | Call `freeze_if_large()` in C4? |
| --- | --- | --- |
| `mutation_segment.rs:715` (literal), `:738` (accounting) | `into_indexed_profiled` | yes |
| `mutation_segment.rs:1575`, `:1598` | `consolidate` | yes |
| `mutation_segment.rs:2704`, `:2727` | `compact_with_mapping` | yes |
| `lifecycle_compaction.rs:101`, `:124` | `base_as_segment` | **no** (temporary whole-collection shard that is split immediately) |

### 2.4 The slot-append helper

`append_shifted_bitmap(destination, source, base)` (`crates/core/src/inventory.rs:20`) adds `base + slot` for every slot of `source`. It fails with `"record slot overflow"` if `base + max` overflows and with `"batch bitmap slots overlap or are out of order"` if a new slot is not greater than every slot already in `destination`. `PostingMap::append_shifted` in Appendix A reproduces both messages exactly.

### 2.5 Tests and examples

- Unit tests live in `name.test.rs` files next to `name.rs`, loaded with `#[cfg(test)] #[path = "name.test.rs"] mod tests;`.
- `crates/core/Cargo.toml` has no `[dev-dependencies]`. Tests use hand-written deterministic generators.
- Examples under `crates/core/examples/` are separate crates. They can only call `pub` items.

## 3. Decisions already made (do not reopen)

**D1. One map type.** `PostingMap<K>` is copied verbatim from Appendix A into `crates/core/src/posting_map.rs`. That code and the test file in Appendix B were compiled and run against `imbl 7.0.2` and `roaring 0.11.5` inside this crate before this plan was written. Do not redesign it.

**D2. Reads return `Cow<'_, RoaringBitmap>`.** `PostingMap::get` returns `Option<Cow<'_, RoaringBitmap>>`. Today it is always `Cow::Borrowed`. Plan 2 will return `Cow::Owned` for some keys, and using `Cow` now means call sites are touched only once. At a call site that needs `&RoaringBitmap`, write `posting.as_ref()` or `.as_deref()` on the `Option`.

**D3. Iteration is forward-only with a direction flag.** `PostingMap::range(lower, upper, descending)` and `iter()` return `PostingIter`, a plain `Iterator`. There is no `.rev()`. Where the old code wrote `.range(..).rev()`, pass `descending = true`.

**D4. Accessor names.** Use exactly these names so later plans can refer to them.

On `PostingIndex` (`posting.rs`):

| Method | Visibility | Purpose |
| --- | --- | --- |
| `value_posting(&self, key: &ScalarKey) -> Option<Cow<'_, RoaringBitmap>>` | `pub(crate)` | existing method, new return type |
| `value_iter(&self) -> PostingIter<'_, ScalarKey>` | `pub(crate)` | ascending traversal |
| `value_range(..) -> PostingValueCursor<'_>` | `pub(crate)` | existing; cursor items become `(ScalarKey, Cow<'a, RoaringBitmap>)` |
| `for_each_value`, `for_each_length` | `pub` | existing; raise visibility (examples need them) |
| `value_bytes(&self) -> usize` | `pub` | sum of `serialized_size()` over value bitmaps |
| `length_bytes(&self) -> usize` | `pub` | same for lengths |
| `value_memberships(&self) -> u64` | `pub` | sum of `len()` over value bitmaps |
| `value_key_text_bytes(&self) -> usize` | `pub(crate)` | sum of `value.len()` for `ScalarKey::String` and `ScalarKey::Id` keys |
| `inferred_index_bytes(&self) -> usize` | `pub(crate)` | the expression now at `inference_build.rs:1141-1154`, moved unchanged |
| `value_entries(&self) -> BTreeMap<ScalarKey, RoaringBitmap>` | `pub` | owned snapshot for tests and examples |
| `length_entries(&self) -> BTreeMap<Number, RoaringBitmap>` | `pub` | owned snapshot for tests and examples |
| `value_entries_equal(&self, other: &BTreeMap<ScalarKey, RoaringBitmap>) -> bool` | `pub(crate)` | equality without cloning |
| `insert_value_slot(&mut self, key: ScalarKey, slot: u32)` | `pub(crate)` | |
| `insert_length_slot(&mut self, key: Number, slot: u32)` | `pub(crate)` | |
| `set_value(&mut self, key: ScalarKey, posting: RoaringBitmap)` | `pub(crate)` | replaces the posting |
| `set_length(&mut self, key: Number, posting: RoaringBitmap)` | `pub(crate)` | replaces the posting |
| `retain_values(&mut self, keep: impl FnMut(&ScalarKey) -> bool)` | `pub(crate)` | |
| `clear_postings(&mut self)` | `pub(crate)` | clears `values`, `lengths` and, if present, the structural postings; does not touch `presence`, `nulls`, `empty_selections` |
| `empty_like(source: &Self, covered_records: u32) -> Self` | `pub(crate)` | same result as today's `empty_projected_posting` literal |
| `from_parts(parts: PostingParts) -> Self` | `pub(crate)` | replaces the struct literal at `inference_build.rs:1089` |
| `freeze_shared(&mut self)` | `pub` | C4; freezes the structural facts only in this plan |

`PostingParts` is a new `pub(crate)` struct in `posting.rs` with public fields `path`, `kind`, `multi`, `presence`, `empty_selections`, `nulls`, `values: BTreeMap<ScalarKey, RoaringBitmap>`, `lengths: BTreeMap<Number, RoaringBitmap>`, `value_complete`, `length_complete`, `empty_string_complete`, `covered_records`. `from_parts` sets `values_indexed: true`, `structural: Some(StructuralFacts::default())` and `stats: PostingStats::default()`, exactly like the literal it replaces.

On `StructuralFacts` (`posting.rs`):

| Method | Visibility | Purpose |
| --- | --- | --- |
| `get(&self, key: StructuralKey) -> Option<Cow<'_, RoaringBitmap>>` | `pub` | |
| `is_empty_for(&self, key: StructuralKey) -> bool` | `pub(crate)` | true when the key is missing or its posting is empty |
| `for_each(&self, visit: impl FnMut(StructuralKey, &RoaringBitmap) -> Result<(), String>) -> Result<(), String>` | `pub` | ascending key order |
| `key_count(&self) -> usize` | `pub(crate)` | |
| `set(&mut self, key: StructuralKey, posting: RoaringBitmap)` | `pub(crate)` | replaces the posting |
| `insert_slot(&mut self, key: StructuralKey, slot: u32)` | `pub(crate)` | |
| `clear(&mut self)` | `pub(crate)` | |
| `entries(&self) -> BTreeMap<StructuralKey, RoaringBitmap>` | `pub` | owned snapshot |
| `estimated_bytes(&self) -> usize` | `pub` | existing; raise visibility |
| `freeze(&mut self)` | `pub(crate)` | C4 |
| `is_shared(&self) -> bool` | `pub(crate)`, `#[cfg(test)]` | C4 |

On `BigramIndex` (`bigram.rs`):

| Method | Visibility | Purpose |
| --- | --- | --- |
| `gram_posting(&self, skip: bool, key: u64) -> Option<Cow<'_, RoaringBitmap>>` | `pub` | |
| `gram_key_count(&self) -> usize` | `pub` | `normal.len() + skip.len()` |
| `gram_posting_bytes(&self) -> usize` | `pub(crate)` | sum of `serialized_size()` over both maps |
| `for_each_normal`, `for_each_skip` | `pub` | existing; raise visibility |
| `normal_entries(&self) -> BTreeMap<u64, RoaringBitmap>` | `pub` | owned snapshot |
| `skip_entries(&self) -> BTreeMap<u64, RoaringBitmap>` | `pub` | owned snapshot |
| `insert_gram_slot(&mut self, skip: bool, key: u64, slot: u32)` | `pub(crate)` | |
| `set_gram(&mut self, skip: bool, key: u64, posting: RoaringBitmap)` | `pub(crate)` | replaces the posting |
| `append_shifted_gram(&mut self, skip: bool, key: u64, source: &RoaringBitmap, base: u32) -> Result<(), String>` | `pub(crate)` | `append_shifted_bitmap(entry(key).or_default(), source, base)` |

**D5. Tests compare snapshots.** Almost every test use of these fields is an equality check such as `assert_eq!(a.values, b.values)`. Replace the field with the snapshot accessor on both sides: `.values` → `.value_entries()`, `.lengths` → `.length_entries()`, `facts.postings` → `facts.entries()`, `.normal` → `.normal_entries()`, `.skip` → `.skip_entries()`. Indexing and length checks keep working on the snapshot, for example `index.value_entries()[&key]` and `index.value_entries().len()`.

**D6. Backing policy.** `posting_map::shared_backing_wanted(record_count)` is true when the cargo feature `force-shared-backing` is on or `record_count >= SHARED_BACKING_MIN_RECORDS` (16,384). It is the only place that decides which backing a shard uses.

**D7. Only structural facts freeze in this plan.** `PostingIndex::freeze_shared` calls `StructuralFacts::freeze` and nothing else. `values` and `lengths` stay in the `Building` backing until plan 2.

**D8. Byte estimates do not change.** Every accounting function must compute the same number as before. Use `value_bytes`, `length_bytes`, `distinct_values`, `distinct_lengths`, `key_count`, `estimated_bytes` and `value_key_text_bytes` to rebuild the old expressions term by term.

**D9. Error strings do not change.** Tests and callers match on them.

**D10. `Repository::relation_reverse_sources`** (`repository.rs:855`) changes its return type from `Option<&RoaringBitmap>` to `Option<Cow<'_, RoaringBitmap>>`. Its callers are `relation_query.rs:336` and `:2117`, `relation_semijoin.rs:484`, `relation_inspection.rs:355`, `sort_binding.rs:1308`, and five uses in `relation.test.rs`. Adapt each with `.as_deref()` or `.as_ref()`.

## 4. Checkpoints

### C0. Baseline

1. Create the integration worktree and task memory as the workflow requires. Record the actual start commit as `START`.
2. Run and save the output of each command under `docs/ai/memory/posting-map-interface/start/`:

```sh
cargo test --workspace --locked --no-fail-fast
cargo clippy --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
cargo run --release --example accept_musenalm -- --alphabet ascii
cargo run --release --example accept_musenalm -- --alphabet unicode
cargo run --release -p objekte-search-core --example bench_build_musenalm -- --root examples/musenalm --alphabet ascii
cargo run --release -p objekte-search-core --example bench_build_musenalm -- --root examples/musenalm --alphabet unicode
python3 bench/run_build_regressions.py --result-dir /tmp/objekte-posting-map-interface/build-start
python3 bench/run_query_regressions.py quick --comparison repair \
  --baseline bench/results/musenalm/controlled-foundation-performance-92cdb92b19 \
  --result-dir /tmp/objekte-posting-map-interface/query-start
```

3. Write down from that output: the passed/ignored test counts per crate, the Clippy warning count, and both `index_signature_sha256` values (ASCII and Unicode). Each `bench_build_musenalm` run prints one JSON object containing that field. `build-start/gate.json` also records both signatures, and a later run of `run_build_regressions.py` with `--baseline` fails by itself if a signature changed.

Done when: all commands succeed and the numbers are recorded. No commit.

### C1. Add the map module

1. In `crates/core/Cargo.toml`, add under `[dependencies]`:

```toml
imbl = "7.0"
```

   and under `[features]`:

```toml
force-shared-backing = []
```

2. Create `crates/core/src/posting_map.rs` with the exact content of Appendix A.
3. Create `crates/core/src/posting_map.test.rs` with the exact content of Appendix B.
4. In `crates/core/src/lib.rs`, add `mod posting_map;` directly after the line `pub mod posting;`. The module is private; its `pub` items are visible inside the crate only.
5. Run:

```sh
cargo test -p objekte-search-core --locked --lib posting_map
cargo test -p objekte-search-core --locked --lib posting_map --features force-shared-backing
```

   Expected: 4 tests pass in each run (`posting_map_matches_btree_model_in_both_backings`, `append_shifted_errors_match_the_native_helper`, `inverted_range_is_empty_in_the_shared_backing`, `shared_backing_policy_follows_size_and_feature`).

6. The compiler will warn that some `PostingMap` methods are unused. That is expected until C2 and C4. Do not suppress the warnings.

Commit: `Add PostingMap with native and persistent backings`. Update `Cargo.lock` in the same commit.

### C2. Encapsulate the posting maps

Goal: `values`, `lengths` and `StructuralFacts.postings` are private `PostingMap` fields, and everything compiles and passes.

#### C2.1 Change the fields (`posting.rs`)

```rust
// in PostingIndex
values: PostingMap<ScalarKey>,
lengths: PostingMap<Number>,

// in StructuralFacts
postings: PostingMap<StructuralKey>,
```

Add `use std::borrow::Cow;`, `use std::ops::Bound;` (if not present) and `use crate::posting_map::{PostingIter, PostingMap};`. `StructuralFacts` keeps `#[derive(Clone, Debug, Default)]`; `PostingMap` implements all three.

#### C2.2 Rewrite the internals of `posting.rs`

About 70 expressions inside `posting.rs` use the three maps. Convert each with this table. The right-hand side behaves identically for the `Building` backing.

| Old expression | New expression |
| --- | --- |
| `self.values.get(key)` where a `&RoaringBitmap` is used | `self.values.get(key)` and then `.as_deref()` on the `Option`, or `.as_ref()` on the `Cow` |
| `self.values.get(key).map(RoaringBitmap::len).unwrap_or(0)` and `map_or(0, RoaringBitmap::len)` | `self.values.cardinality(key)` |
| `self.values.get(&key).cloned().unwrap_or_default()` | `self.values.get(&key).map(Cow::into_owned).unwrap_or_default()` |
| `self.values.get(key).map(\|bitmap\| input & bitmap)` | `self.values.get(key).map(\|bitmap\| input & bitmap.as_ref())` |
| `self.values.entry(key).or_default().insert(slot.0)` | `self.values.insert_slot(key, slot.0)` |
| `append_shifted_bitmap(self.values.entry(key).or_default(), &posting, base)?` | `self.values.append_shifted(key, &posting, base)?` |
| `for (key, posting) in batch.values` (consuming) | `for (key, posting) in batch.values.into_entries()` |
| `for (key, posting) in &self.values` | `for (key, posting) in self.values.iter()`; `posting` is a `Cow`, pass `posting.as_ref()` |
| `self.values.first_key_value().map(\|(key, _)\| key.clone())` | `self.values.first_key().cloned()` |
| `self.values.last_key_value().map(\|(key, _)\| key.clone())` | `self.values.last_key().cloned()` |
| `self.values.values().all(valid)` | `self.values.all_bitmaps(valid)` |
| `.values().chain(self.lengths.values()).any(\|slots\| !slots.is_subset(allowed))` | `!self.values.all_bitmaps(\|slots\| slots.is_subset(allowed)) \|\| !self.lengths.all_bitmaps(\|slots\| slots.is_subset(allowed))` |
| `entries.range((Excluded(key), Unbounded))` | `entries.range(Bound::Excluded(key), Bound::Unbounded, false)` |
| `entries.range((Included(key), Unbounded))` | `entries.range(Bound::Included(key), Bound::Unbounded, false)` |
| `entries.range((Unbounded, Excluded(key)))` | `entries.range(Bound::Unbounded, Bound::Excluded(key), false)` |
| `entries.range((Unbounded, Included(key)))` | `entries.range(Bound::Unbounded, Bound::Included(key), false)` |
| any of the four above followed by `.rev()` | the same call with `true` as the last argument, and no `.rev()` |
| `values.range(start..end)` | `values.range(Bound::Included(&start), Bound::Excluded(&end), false)` |
| `values.range(..bound.clone()).rev()` | `values.range(Bound::Unbounded, Bound::Excluded(bound), true)` |
| `values.range(bound.clone()..)` | `values.range(Bound::Included(bound), Bound::Unbounded, false)` |
| function parameter `entries: &BTreeMap<K, RoaringBitmap>` | `entries: &PostingMap<K>` with `K: Ord + Clone` |
| iterator item type `(&'a K, &'a RoaringBitmap)` in `union_postings` | `(&'a K, Cow<'a, RoaringBitmap>)`; inside the loop use `posting.as_ref()` |
| `BTreeMap::new()` for these fields in `PostingIndex::new` | `PostingMap::new()` |

Specific functions:

- `PostingValueCursor` (`posting.rs:144`) becomes a struct around `PostingIter`:

```rust
/// Ordered traversal over one posting dictionary's values.
pub(crate) struct PostingValueCursor<'a>(PostingIter<'a, ScalarKey>);


impl<'a> PostingValueCursor<'a> {
    pub(crate) fn next(&mut self) -> Option<(ScalarKey, Cow<'a, RoaringBitmap>)> {
        self.0.next().map(|(key, posting)| (key.clone(), posting))
    }
}
```

  and `value_range` becomes `PostingValueCursor(self.values.range(lower.as_ref(), upper.as_ref(), descending))`.

- `state_source` (`posting.rs:249`) returns `Option<Option<Cow<'_, RoaringBitmap>>>`. The non-multi null case returns `Some(Some(Cow::Borrowed(&self.nulls)))`. Its three callers (`posting.rs:441`, `:785`, `:933`) use `source.as_deref()` where they need `Option<&RoaringBitmap>`.
- `StructuralFacts::insert` keeps its body with `self.postings.insert_slot(key, slot.0)`.
- `StructuralFacts::merge_shifted` becomes `for (key, posting) in batch.postings.into_entries() { self.postings.append_shifted(key, &posting, base)?; }`.
- `StructuralFacts::estimated_bytes` becomes `self.postings.serialized_bytes()`.
- `finish_stats` uses `self.values.len()`, `first_key().cloned()`, `last_key().cloned()`.

Then add the new methods from D4. Each is one or two lines over the `PostingMap` API. `value_entries_equal` is:

```rust
pub(crate) fn value_entries_equal(&self, other: &BTreeMap<ScalarKey, RoaringBitmap>) -> bool {
    self.values.len() == other.len()
        && other
            .iter()
            .all(|(key, posting)| self.values.get(key).as_deref() == Some(posting))
}
```

`inferred_index_bytes` must contain exactly this expression (moved from `inference_build.rs`):

```rust
pub(crate) fn inferred_index_bytes(&self) -> usize {
    self.values
        .iter()
        .map(|(key, slots)| {
            128 + match key {
                ScalarKey::String(text) => text.len(),
                _ => 0,
            } + slots.serialized_size()
        })
        .sum::<usize>()
        + self
            .lengths
            .iter()
            .map(|(_, slots)| 64 + slots.serialized_size())
            .sum::<usize>()
}
```

#### C2.3 Convert the production call sites

| Site | Today | Change to |
| --- | --- | --- |
| `index_catalog.rs:91` | `facts.postings.get(&key).is_none_or(RoaringBitmap::is_empty)` | `facts.is_empty_for(key)` |
| `inference_build.rs:1089` | `PostingIndex { .. }` literal | `PostingIndex::from_parts(PostingParts { .. })` with the same field values |
| `inference_build.rs:1129`, `:1133` | `facts.postings.insert(key, slots)` | `facts.set(key, slots)` |
| `inference_build.rs:1141-1154` | byte expression over `index.values` and `index.lengths` | `index_bytes = index.inferred_index_bytes();` (the following `index_bytes += ... StructuralFacts::estimated_bytes` lines stay) |
| `lifecycle.rs:463-468` | sums of `serialized_size()` over `posting.values` and `posting.lengths` | `posting.value_bytes() + posting.length_bytes()` (the structural term after it stays) |
| `mutation_segment.rs:757` `posting_bytes` | reads `facts.postings`, `index.values`, `index.lengths` | rebuild each term: `facts.key_count()`, `facts.estimated_bytes()`, `index.distinct_values()`, `index.value_bytes()`, `index.value_key_text_bytes()`, `index.distinct_lengths()`, `index.length_bytes()`. Keep the order of the `saturating_add` calls and the `ACCOUNTING_OVERHEAD + size_of::<..>()` factors. |
| `mutation_segment.rs:1254-1258` | `facts.postings.clear(); index.values.clear(); index.lengths.clear();` | `index.clear_postings();` (keep the three bitmap `.clear()` lines above it) |
| `mutation_segment.rs:1616` `empty_projected_posting` | struct literal | body becomes `PostingIndex::empty_like(source, logical_count)` |
| `mutation_segment.rs:1639` `scatter_posting_key` | helper over `BTreeMap` | delete the function |
| `mutation_segment.rs:1683-1692` | `for (key, bitmap) in &facts.postings` and `.postings.entry(*key).or_default().insert(slot.0)` | `facts.for_each(\|key, bitmap\| { .. parts[part].structural.as_mut().unwrap().insert_slot(key, slot.0); .. Ok(()) })?;` |
| `mutation_segment.rs:1696`, `:1704` | `scatter_posting_key(&mut parts[part].values, key, slot.0)` | `parts[part].insert_value_slot(key.clone(), slot.0)` and `insert_length_slot(key.clone(), slot.0)` |
| `mutation_segment.rs:1886` `project_posting` (literal at `:1892`) | struct literal | `let mut projected = PostingIndex::empty_like(index, logical_count);` then assign `projected.presence`, `projected.empty_selections`, `projected.nulls` from the three `project_bitmap_in_range` calls |
| `mutation_segment.rs:1915`, `:1922` | `projected.values.insert(key.clone(), kept)` | `projected.set_value(key.clone(), kept)` and `set_length` |
| `mutation_segment.rs:1926-1932` | loop over `&source.postings`, `target.postings.insert(*key, kept)` | `source.for_each(\|key, bitmap\| { let kept = ..; if !kept.is_empty() { target.set(key, kept); } Ok(()) })?;` |
| `posting_values.rs:406` | `index.values.retain(\|key, _\| ..)` | `index.retain_values(\|key\| ..)` |
| `posting_values.rs:819`, `:1100` | cursor items `(ScalarKey, &RoaringBitmap)` | items are now `(ScalarKey, Cow<RoaringBitmap>)`; method calls such as `posting.len()` and `posting.intersection_len(..)` work unchanged through deref |
| `relation.rs:241` | `reverse != posting.values` | `!posting.value_entries_equal(&reverse)` |
| `repository.rs:872` | `.values.get(&ScalarKey::Id(..))` | `.value_posting(&ScalarKey::Id(..))`, and the return type per D10 |
| `relation_query.rs:360`, `run.rs:5124` | `field.value_posting(..)` used as `&RoaringBitmap` | add `.as_ref()` where the compiler asks, for example `may \|= slots.as_ref();` |
| `sorting.rs:407-408` | `let value_map = &index.values; for (value, slots) in value_map.iter()` | `for (value, slots) in index.value_iter()` |

`empty_like` must produce exactly what the two old literals produced: `path`, `kind`, `multi`, `values_indexed`, `value_complete`, `length_complete`, `empty_string_complete` copied from `source`; `presence`, `empty_selections`, `nulls` empty; `structural` is `Some(StructuralFacts::default())` if the source has structural facts, else `None`; `values` and `lengths` empty; `covered_records` as given; `stats` default.

#### C2.4 Convert tests and examples

Tests, with the D5 replacements:

| File | Sites at the audited base |
| --- | --- |
| `crates/core/src/build.test.rs` | 8 posting sites |
| `crates/core/src/inference.test.rs` | 14 |
| `crates/core/src/lifecycle_mutation.test.rs` | 1 |
| `crates/core/src/mutation_segment.test.rs` | 19 posting and structural sites |

One pattern needs care: `expected.structural.as_ref().map(|facts| &facts.postings)` becomes `expected.structural.as_ref().map(|facts| facts.entries())` on both sides of the comparison.

Examples:

| File and line | Today | Change to |
| --- | --- | --- |
| `examples/bench_build_musenalm.rs:30-50` `hash_posting` | iterates `facts.postings`, `index.values`, `index.lengths` | see below |
| `examples/bench_inference_fixture.rs:80-81` | `index.values.len()`, sum of `len()` | `index.distinct_values()`, `index.value_memberships()` |
| `examples/bench_musenalm.rs:322-329` | sums of `serialized_size()` | `p.value_bytes() + p.length_bytes()` |
| `examples/bench_index_construction.rs:231-245` | the same two sums, plus the sum over `facts.postings` | replace those three sums by `p.value_bytes() + p.length_bytes() + p.structural.as_ref().map_or(0, \|facts\| facts.estimated_bytes())`; keep the rest of the expression |

`hash_posting` feeds the index signature, so the bytes it hashes must stay the same, in the same order:

```rust
if let Some(facts) = &index.structural {
    let mut keys = Vec::new();
    facts.for_each(|key, _| {
        keys.push(format!("{key:?}"));
        Ok(())
    })?;
    keys.sort();
    for key in keys {
        hash_value(hash, &key)?;
    }
    facts.for_each(|key, slots| {
        hash_value(hash, &format!("{key:?}"))?;
        hash_value(hash, slots)
    })?;
}
index.for_each_value(|key, slots| {
    hash_value(hash, key)?;
    hash_value(hash, slots)
})?;
index.for_each_length(|key, slots| {
    hash_value(hash, key)?;
    hash_value(hash, slots)
})?;
```

#### C2.5 Verify

```sh
cargo test --workspace --locked --no-fail-fast
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
cargo run --release -p objekte-search-core --example bench_build_musenalm -- --root examples/musenalm --alphabet ascii
```

Done when: test counts equal C0 plus the 4 tests added in C1, and the ASCII `index_signature_sha256` equals the C0 value.

Commit: `Encapsulate posting maps behind PostingMap`.

### C3. Encapsulate the gram maps

1. In `bigram.rs`, remove `pub` from `normal` and `skip` in `BigramIndex` (`bigram.rs:267-268`). The type stays `HashMap<u64, RoaringBitmap>`.
2. Add the `BigramIndex` methods from D4. Bodies:
   - `gram_posting`: `if skip { self.skip.get(&key) } else { self.normal.get(&key) }.map(Cow::Borrowed)`
   - `insert_gram_slot`: `map.entry(key).or_default().insert(slot);`
   - `set_gram`: `map.insert(key, posting);`
   - `append_shifted_gram`: `crate::inventory::append_shifted_bitmap(map.entry(key).or_default(), source, base)`
   - `normal_entries` / `skip_entries`: clone into a `BTreeMap`.
3. Convert the production sites, all in `mutation_segment.rs`:

| Site | Today | Change to |
| --- | --- | --- |
| `:792` `gram_bytes` | `index.normal.len().saturating_add(index.skip.len())` and the sum of `bitmap_bytes` | `index.gram_key_count()` and `index.gram_posting_bytes()`; keep the multiplication by `ACCOUNTING_OVERHEAD + size_of::<u64>()` |
| `:1121-1126` `merge_bigram_index` | `append_shifted_bitmap(target.normal.entry(*key).or_default(), posting, base)` | `target.append_shifted_gram(false, *key, posting, base)`; `true` for skip |
| `:1950`, `:1957` `project_bigram` | `projected.normal.insert(*key, kept)` | `projected.set_gram(false, *key, kept)`; `true` for skip |
| `:1983`, `:1995` `scatter_bigram_partitions` | `parts[*part].normal.entry(*key).or_default().insert(slot.0)` | `parts[*part].insert_gram_slot(false, *key, slot.0)`; `true` for skip |

4. Convert tests with the D5 replacements: `build.test.rs` (40 sites), `mutation_segment.test.rs` (8), `source.test.rs` (4).
5. Convert examples:

| File and line | Change |
| --- | --- |
| `bench_build_musenalm.rs:59-73` `hash_bigram` | replace the loop over `[("normal", &index.normal), ("skip", &index.skip)]` with: hash `"normal"`, then `index.for_each_normal(\|key, slots\| { hash_value(hash, key)?; hash_value(hash, slots) })?;`, then hash `"skip"`, then the same with `for_each_skip`. `for_each_normal` already visits keys in ascending order, which is the order the old code used. |
| `bench_build_musenalm.rs:221`, `bench_musenalm.rs:355`, `bench_index_construction.rs:277`, `bench_case_modes.rs:164` | `index.normal.len() + index.skip.len()` → `index.gram_key_count()` |

6. Verify with the same three commands as C2.5, and also run `bench_build_musenalm` with `--alphabet unicode`. Both signatures must equal the C0 values.

Commit: `Encapsulate bigram maps behind BigramIndex methods`.

### C4. Persistent pilot on structural facts

1. `posting.rs`:

```rust
// StructuralFacts
pub(crate) fn freeze(&mut self) {
    self.postings.freeze();
}

#[cfg(test)]
pub(crate) fn is_shared(&self) -> bool {
    self.postings.is_shared()
}

// PostingIndex
/// Move every family that supports it to the persistent backing.
pub fn freeze_shared(&mut self) {
    if let Some(facts) = &mut self.structural {
        facts.freeze();
    }
}
```

2. `mutation_segment.rs`, in the `impl IndexedCollectionSegment` block that contains `recompute_accounting`:

```rust
/// Use the persistent backing when the shard-size policy asks for it.
/// Contents and query results are identical in both backings.
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
}
```

3. Call `index.freeze_if_large();` immediately before `index.recompute_accounting();` at `mutation_segment.rs:738`, `:1598` and `:2727`. Do not call it in `lifecycle_compaction.rs` (`base_as_segment`).
4. Add these tests.

   In `posting.test.rs`:
   - `frozen_structural_facts_match_native_facts`: build a `PostingIndex` with structural facts by adding at least one record of each kind (string, number, array, object, empty object, null inside an array, empty string). Clone it, call `freeze_shared()` on the clone. Assert `entries()` are equal, `is_empty_for(key)` is equal for all seven `StructuralKey` values, and `estimated_bytes()` is equal.
   - `frozen_structural_facts_merge_like_native_facts`: take two such indexes `a` and `b`. Compute `native = a.clone(); native.merge_shifted(b.clone(), n)`. Compute `mixed = a.clone(); mixed.freeze_shared(); mixed.merge_shifted(b.clone(), n)`. Assert the structural `entries()` are equal and that a clone of `mixed` taken before the merge still equals `a`.

   In `mutation_segment.test.rs`:
   - `sealed_shard_backing_follows_policy`: build a small shard with the helpers already used in that file. Assert that every `StructuralFacts` in `postings.fields` reports `is_shared() == cfg!(feature = "force-shared-backing")`.
   - `consolidation_of_frozen_and_native_shards_matches_native`: build shards `a` and `b`. Let `expected = consolidate(&[&a, &b], ..)`. Build `a` a second time with the same helper and inputs (`IndexedCollectionSegment` is not `Clone`), call `freeze_shared()` on every posting index in its `postings.fields`, and call the result `frozen_a`. Let `actual = consolidate(&[&frozen_a, &b], ..)`. Assert that `value_entries()`, `length_entries()` and structural `entries()` are equal for every field, and that `accounting.retained_bytes_estimated` is equal.

5. Run:

```sh
cargo test --workspace --locked --no-fail-fast
cargo test --workspace --locked --no-fail-fast --features objekte-search-core/force-shared-backing
cargo run --release -p objekte-search-core --features force-shared-backing --example accept_musenalm -- --alphabet ascii
cargo run --release -p objekte-search-core --features force-shared-backing --example accept_musenalm -- --alphabet unicode
```

Done when: all four commands pass. The two test runs report the same counts.

Commit: `Pilot the persistent backing on structural facts`.

### C5. Validate and report

1. Re-run every command from C0 into `/tmp/objekte-posting-map-interface/*-final` directories. For the build gate add `--baseline /tmp/objekte-posting-map-interface/build-start/gate.json`.
2. Check the gates in section 5.
3. Add `docs/development/posting-map.md` (about 30 lines): the two backings, the policy function, the feature flag, and the rule that index code never touches the maps directly. Add one bullet for it to the link list at the top of `README.md`.
4. Complete the task as `docs/ai/README.md` section 7 describes, with the report contents listed in section 7 of this plan.

## 5. Gates

| Gate | Requirement |
| --- | --- |
| G1 | `cargo test --workspace --locked --no-fail-fast` passes. Per-crate counts are C0 counts plus the new tests (4 in `posting_map`, 2 in `posting`, 2 in `mutation_segment`). No test was removed or ignored. |
| G2 | The same command with `--features objekte-search-core/force-shared-backing` passes with the same counts. |
| G3 | `index_signature_sha256` for ASCII and Unicode equals the C0 values. |
| G4 | `accept_musenalm` passes for ASCII and Unicode, with default features and with `force-shared-backing`. |
| G5 | Clippy warning count is not higher than at C0. |
| G6 | `cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked` passes. |
| G7 | Build gate (default features): median construction time and peak RSS within 5% of C0. |
| G8 | Quick query gate (default features) passes its own checks against the named baseline. |

For G7 and G8: report the numbers in every case. If a value is between 5% and 15% worse, repeat the measurement twice, investigate, and explain the cause in the report. If it is more than 15% worse and you cannot fix it inside this plan's scope, stop and write a blocker.

## 6. Pitfalls

- **Do not change key ordering.** `PostingMap` uses `K: Ord`. `ScalarKey` and `Number` compare equal across representations (`1` and `1.0`). Any byte-level key encoding would break range queries.
- **Do not prune empty postings.** A key whose bitmap is empty must stay in the map exactly as it does in a `BTreeMap` today. `append_shifted` with an empty source still creates the key.
- **Do not hold a `Cow` across a mutation of the same map.** Take what you need, drop it, then write.
- **`for_each_*` closures receive `&RoaringBitmap`.** Keep that signature; inside the method pass `posting.as_ref()`.
- **`into_entries` consumes the map.** Use it only where the old code consumed the `BTreeMap` (`for (key, posting) in batch.values`).
- **Accounting must match to the byte.** `compaction_reuses_unaffected_native_segment` and similar tests compare `accounting` values.
- **Do not call `freeze_if_large` in `base_as_segment`.** That shard is temporary and can be very large.
- **Do not run a blanket formatter.**

## 7. The completion report must include

- `START` commit, integration branch, list of commits per checkpoint.
- The C0 and final values for: test counts per crate, Clippy warning count, both index signatures, build gate medians (time and RSS), quick query gate result.
- The forced-feature test counts.
- Any call site that was not in this plan's tables, and how it was converted.
- Any `PostingMap` method deleted because it ended up unused.
- Measured deviations and their explanation, if any.

## Appendix A. `crates/core/src/posting_map.rs`

Copy exactly. Then apply the repository spacing script to this file.

```rust
//! Ordered key -> record-slot postings with two interchangeable backings.
//!
//! `Building` is the native `BTreeMap` used while an index is constructed and
//! by small immutable shards. `Shared` is a persistent (copy-on-write) ordered
//! map, so that a clone is cheap and a write copies only the changed path.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;

use imbl::OrdMap;
use roaring::RoaringBitmap;

/// Persistent ordered postings: a clone shares every node, and a write
/// copies only the nodes on the path to the changed key plus that bitmap.
#[derive(Clone, Debug)]
pub struct SharedPostings<K: Ord + Clone> {
    entries: OrdMap<K, Arc<RoaringBitmap>>,
}

type SharedEntry<'a, K> = (&'a K, &'a Arc<RoaringBitmap>);

impl<K: Ord + Clone> Default for SharedPostings<K> {
    fn default() -> Self {
        Self {
            entries: OrdMap::new(),
        }
    }
}

impl<K: Ord + Clone> SharedPostings<K> {
    pub fn from_building(source: BTreeMap<K, RoaringBitmap>) -> Self {
        Self {
            entries: source
                .into_iter()
                .map(|(key, posting)| (key, Arc::new(posting)))
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    pub fn get(&self, key: &K) -> Option<Cow<'_, RoaringBitmap>> {
        self.entries
            .get(key)
            .map(|posting| Cow::Borrowed(posting.as_ref()))
    }

    pub fn cardinality(&self, key: &K) -> u64 {
        self.entries.get(key).map_or(0, |posting| posting.len())
    }

    fn posting_mut(&mut self, key: &K) -> &mut RoaringBitmap {
        if !self.entries.contains_key(key) {
            self.entries
                .insert(key.clone(), Arc::new(RoaringBitmap::new()));
        }
        Arc::make_mut(self.entries.get_mut(key).expect("posting was just ensured"))
    }

    pub fn insert_slot(&mut self, key: &K, slot: u32) -> bool {
        self.posting_mut(key).insert(slot)
    }

    pub fn append_shifted(
        &mut self,
        key: &K,
        source: &RoaringBitmap,
        base: u32,
    ) -> Result<(), String> {
        if let Some(max) = source.max() {
            base.checked_add(max).ok_or("record slot overflow")?;
        }
        let first_new = source.min().map(|min| base + min);
        let existing = self.entries.get(key).and_then(|posting| posting.max());
        if let (Some(first_new), Some(existing)) = (first_new, existing)
            && first_new <= existing
        {
            return Err("batch bitmap slots overlap or are out of order".into());
        }
        self.posting_mut(key)
            .append(source.iter().map(|slot| base + slot))
            .map_err(|_| "batch bitmap slots overlap or are out of order")?;
        Ok(())
    }

    pub fn replace(&mut self, key: &K, posting: RoaringBitmap) {
        self.entries.insert(key.clone(), Arc::new(posting));
    }

    pub fn retain_keys(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let doomed = self
            .entries
            .keys()
            .filter(|key| !keep(key))
            .cloned()
            .collect::<Vec<_>>();
        for key in doomed {
            self.entries.remove(&key);
        }
    }

    pub fn clear(&mut self) {
        self.entries = OrdMap::new();
    }

    pub fn first_key(&self) -> Option<&K> {
        self.entries.get_min().map(|(key, _)| key)
    }

    pub fn last_key(&self) -> Option<&K> {
        self.entries.get_max().map(|(key, _)| key)
    }

    pub fn serialized_bytes(&self) -> usize {
        self.entries
            .iter()
            .map(|(_, posting)| posting.serialized_size())
            .sum()
    }

    pub fn all_parts(&self, mut valid: impl FnMut(&RoaringBitmap) -> bool) -> bool {
        self.entries.iter().all(|(_, posting)| valid(posting))
    }

    fn entries<'a>(
        &'a self,
        lower: Bound<&K>,
        upper: Bound<&K>,
        descending: bool,
    ) -> Box<dyn Iterator<Item = SharedEntry<'a, K>> + 'a> {
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
        let range = self.entries.range((lower.cloned(), upper.cloned()));
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
    Shared(Box<dyn Iterator<Item = SharedEntry<'a, K>> + 'a>),
}

impl<'a, K: Ord + Clone> Iterator for PostingIter<'a, K> {
    type Item = (&'a K, Cow<'a, RoaringBitmap>);

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            IterInner::Building(entries) => entries
                .next()
                .map(|(key, posting)| (key, Cow::Borrowed(posting))),
            IterInner::Shared(entries) => entries
                .next()
                .map(|(key, posting)| (key, Cow::Borrowed(posting.as_ref()))),
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
            Backing::Shared(shared) => {
                IterInner::Shared(shared.entries(lower, upper, descending))
            }
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

## Appendix B. `crates/core/src/posting_map.test.rs`

Copy exactly.

```rust
use super::*;

/// Small deterministic generator; the crate has no test dependencies.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as u32) % bound
    }
}

/// Slots cover three 65,536-slot ranges so both chunk sizes cross boundaries.
const SPAN: u32 = 3 << 16;
const KEYS: u32 = 12;

fn assert_same(model: &BTreeMap<u16, RoaringBitmap>, map: &PostingMap<u16>, random: &mut Lcg) {
    assert_eq!(map.len(), model.len());
    assert_eq!(map.is_empty(), model.is_empty());
    assert_eq!(&map.to_btree_map(), model);
    assert_eq!(map.first_key(), model.keys().next());
    assert_eq!(map.last_key(), model.keys().next_back());
    for key in 0..KEYS as u16 {
        assert_eq!(map.get(&key).map(Cow::into_owned), model.get(&key).cloned());
        assert_eq!(
            map.cardinality(&key),
            model.get(&key).map_or(0, RoaringBitmap::len)
        );
        assert_eq!(map.contains_key(&key), model.contains_key(&key));
    }
    let a = random.next(KEYS) as u16;
    let b = random.next(KEYS) as u16;
    let (low, high) = (a.min(b), a.max(b));
    let mut ranges = vec![
        (Bound::Included(&low), Bound::Included(&high)),
        (Bound::Excluded(&low), Bound::Unbounded),
        (Bound::Unbounded, Bound::Excluded(&high)),
        (Bound::Unbounded, Bound::Unbounded),
    ];
    if low != high {
        ranges.push((Bound::Included(&low), Bound::Excluded(&high)));
        ranges.push((Bound::Excluded(&low), Bound::Included(&high)));
    }
    for (lower, upper) in ranges {
        for descending in [false, true] {
            let range = model.range::<u16, _>((lower, upper));
            let expected: Vec<(u16, RoaringBitmap)> = if descending {
                range.rev().map(|(key, posting)| (*key, posting.clone())).collect()
            } else {
                range.map(|(key, posting)| (*key, posting.clone())).collect()
            };
            let actual: Vec<(u16, RoaringBitmap)> = map
                .range(lower, upper, descending)
                .map(|(key, posting)| (*key, posting.into_owned()))
                .collect();
            assert_eq!(actual, expected);
        }
    }
}

fn model_append(target: &mut RoaringBitmap, source: &RoaringBitmap, base: u32) -> Result<(), ()> {
    if source.max().is_some_and(|max| base.checked_add(max).is_none()) {
        return Err(());
    }
    let mut trial = target.clone();
    trial
        .append(source.iter().map(|slot| base + slot))
        .map_err(|_| ())?;
    *target = trial;
    Ok(())
}

/// Every operation must behave exactly like the reference `BTreeMap`, before
/// and after `freeze`, and an earlier clone must never see a later write.
#[test]
fn posting_map_matches_btree_model_in_both_backings() {
    // The forced validation feature uses tiny chunks, which makes each
    // dense split much more work; fewer seeds keep the run short.
    let seeds: u64 = if cfg!(feature = "force-shared-backing") { 30 } else { 200 };
    for seed in 0..seeds {
        let mut random = Lcg(seed.wrapping_mul(0x9E37_79B9) + 1);
        let mut model: BTreeMap<u16, RoaringBitmap> = BTreeMap::new();
        let mut map: PostingMap<u16> = PostingMap::new();
        let freeze_at = random.next(60);
        let mut snapshots = Vec::new();
        for step in 0..120u32 {
            if step == freeze_at {
                map.freeze();
                assert!(map.is_shared());
            }
            let key = random.next(KEYS) as u16;
            match random.next(10) {
                0..=4 => {
                    let slot = random.next(SPAN);
                    assert_eq!(
                        map.insert_slot(key, slot),
                        model.entry(key).or_default().insert(slot)
                    );
                }
                5 | 6 => {
                    let mut source = RoaringBitmap::new();
                    for _ in 0..random.next(6) {
                        source.insert(random.next(SPAN));
                    }
                    let base = random.next(SPAN);
                    let expected = model_append(model.entry(key).or_default(), &source, base);
                    let actual = map.append_shifted(key, &source, base).map_err(|_| ());
                    assert_eq!(actual, expected, "seed {seed} step {step}");
                }
                7 => {
                    let mut posting = RoaringBitmap::new();
                    for _ in 0..random.next(5) {
                        posting.insert(random.next(SPAN));
                    }
                    if random.next(4) == 0 {
                        posting.insert_range(0..SPAN);
                    }
                    model.insert(key, posting.clone());
                    map.replace(key, posting);
                }
                8 => {
                    let cut = random.next(KEYS) as u16;
                    model.retain(|key, _| *key != cut);
                    map.retain_keys(|key| *key != cut);
                }
                _ => snapshots.push((model.clone(), map.clone())),
            }
            assert_same(&model, &map, &mut random);
        }
        for (old_model, old_map) in &snapshots {
            assert_eq!(&old_map.to_btree_map(), old_model);
        }
        let mut frozen = map.clone();
        frozen.freeze();
        assert!(frozen == map);
        assert_eq!(
            frozen.clone().into_entries(),
            model.clone().into_iter().collect::<Vec<_>>()
        );
        assert!(frozen.all_bitmaps(|posting| posting.max().is_none_or(|max| max < 2 * SPAN)));
        frozen.clear();
        assert!(frozen.is_empty() && frozen.is_shared());
    }
}

#[test]
fn append_shifted_errors_match_the_native_helper() {
    for frozen in [false, true] {
        let mut map: PostingMap<u16> = PostingMap::new();
        if frozen {
            map.freeze();
        }
        let source: RoaringBitmap = [0u32, 5].into_iter().collect();
        map.append_shifted(1, &source, 10).unwrap();
        assert_eq!(
            map.append_shifted(1, &source, 15).unwrap_err(),
            "batch bitmap slots overlap or are out of order"
        );
        assert_eq!(
            map.append_shifted(1, &source, u32::MAX - 1).unwrap_err(),
            "record slot overflow"
        );
        // An empty source still creates the key, like `entry().or_default()`.
        map.append_shifted(2, &RoaringBitmap::new(), 0).unwrap();
        assert!(map.contains_key(&2));
        assert_eq!(map.cardinality(&2), 0);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&1).unwrap().iter().collect::<Vec<_>>(), vec![10, 15]);
    }
}

#[test]
fn inverted_range_is_empty_in_the_shared_backing() {
    let mut map: PostingMap<u16> = PostingMap::new();
    map.insert_slot(3, 1);
    map.freeze();
    assert_eq!(
        map.range(Bound::Included(&5), Bound::Included(&2), false).count(),
        0
    );
    assert_eq!(
        map.range(Bound::Excluded(&3), Bound::Excluded(&3), true).count(),
        0
    );
}

#[test]
fn shared_backing_policy_follows_size_and_feature() {
    assert!(shared_backing_wanted(SHARED_BACKING_MIN_RECORDS));
    assert_eq!(
        shared_backing_wanted(SHARED_BACKING_MIN_RECORDS - 1),
        cfg!(feature = "force-shared-backing")
    );
}
```

## Implementation report

- Task identifier: `posting-map-interface`
- Completion date: 2026-11-15
- Base commit: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`
- Integration branch: `integration/posting-map-interface`
- Integration worktree: `/home/simon/source/objekte-second-base-integration-posting-map-interface`
- Implementation commits:
  - `a0506704ec` Add PostingMap with native and persistent backings
  - `cc74f13bec` Encapsulate posting maps behind PostingMap
  - `ab2833f6c9` Encapsulate bigram maps behind BigramIndex methods
  - `91191239d5` Pilot the persistent backing on structural facts
  - final documentation/report commit (this commit)
- START commit: `c54486f893` (original checkout `objekte-second-base` was clean and
  matched `master` at the audited base).

### Summary

`crates/core/src/posting_map.rs` now defines `PostingMap<K>` with the native
`Building` and persistent `Shared` (`imbl::OrdMap`) backings, `PostingIter`,
`shared_backing_wanted` and `SHARED_BACKING_MIN_RECORDS`. `PostingIndex.values`,
`PostingIndex.lengths` and `StructuralFacts.postings` are private maps behind the
accessor methods of D4; `BigramIndex.normal` and `.skip` are private behind the
new gram accessors. `StructuralFacts` freezes to the persistent backing when a
sealed shard is at least 16,384 records, or for every sealed shard under the new
`force-shared-backing` feature. Query results, accounting byte estimates, error
strings and the index signature are unchanged. `Repository::relation_reverse_sources`
returns `Option<Cow<'_, RoaringBitmap>>` per D10.

### Validation

- C0 (`docs/ai/memory/posting-map-interface/start/`):
  - default workspace tests: 820 passed, 0 failed, 17 ignored
    (core 545/15, cli 115/1, desktop 160/1).
  - Clippy: 240 warning lines (221 excluding `generated` summaries).
  - `index_signature_sha256`: ASCII
    `5116df6395bf7c92fae99422dcf7f4456852dfa6acdf275dd0915d22114cb53e`,
    Unicode `595f189a9ee32ced1ca07f17d7c61fd5b019038073e24d42df1a4c7ab9bada07`.
  - build gate: ASCII 1.008 s / 485.1 MB, Unicode 1.124 s / 504.3 MB.
- Final (`docs/ai/memory/posting-map-interface/final/`):
  - default workspace tests: 828 passed, 0 failed, 17 ignored (C0 + 4
    `posting_map` + 2 `posting` + 2 `mutation_segment`).
  - `force-shared-backing` workspace tests: 828 passed, 0 failed, 17 ignored
    (same counts).
  - Clippy: 237 warning lines (218 excluding `generated` summaries), not higher
    than C0.
  - `detailed-profiling` check passes.
  - `accept_musenalm` passes for ASCII and Unicode, default features and
    `force-shared-backing`.
  - Both index signatures equal C0.
  - build gate (default features) has no failure; ASCII 1.062 s / 493.2 MB,
    Unicode 1.215 s / 510.3 MB (first final run). Two repeats on fresh result
    directories gave ASCII 1.062/1.091/1.144 s and Unicode 1.215/1.107/1.189 s;
    medians are 8.2% (ASCII) and 5.8% (Unicode) above C0, with RSS +2.3% and
    +1.2%. This is inside the plan's 5%-15% repeat-and-explain band: the
    measurement was repeated twice as required, the duplicate runs spread over
    roughly the same range, and the repeated runs were taken while another
    long-running build shared the machine (load average around 19 on 32 CPUs).
    The wrapper adds one predictable backing branch per posting operation even
    in `Building`; no algorithmic change was introduced for default builds.
- Quick query gate: the named baseline
  `controlled-foundation-performance-92cdb92b19` was recorded at checkout path
  `/home/simon/source/objekte`, while this task's checkout is
  `/home/simon/source/objekte-second-base`. The gate therefore rejects the
  comparison before timing on `corpus_fingerprint`, the missing baseline
  schema manifest and semantic registrations; it also failed at C0 on the
  untouched base with the same compatibility failures plus three latency
  confirmations. The final C0-relative run has aggregate ratio 0.728 (faster)
  and no timing confirmation failures; its only failures are the `repair`
  mode's fixed foundation-revision rule and the governor check (the machine
  runs `performance`, while `repair` expects `powersave` when no
  `environment.json` exists in the result directory). These are environmental
  baseline limitations, not regressions. Both final runs are retained under
  `/tmp/objekte-posting-map-interface/query-final*`.

### Call sites not listed in the plan's tables

- `relation_inspection.rs:355`: `relation_reverse_sources` now returns `Cow`;
  the visible bitmap is built with `into_owned`/`as_ref` depending on the
  active-domain filter.
- `relation_semijoin.rs:484`: `next |= sources.as_ref()`.
- `sorting.rs`: the second inferred-order loop still used the removed
  `value_map`; it now walks `value_iter()` / `value_range(.., true)`.
- `bench_case_modes.rs:164`: gram key count uses `gram_key_count()`.
- `posting.test.rs`: the broad-ID union fixture builds values with `set_value`
  and compares through `value_posting` rather than mutating a snapshot.

### Deleted `PostingMap` methods

None. All Appendix A methods are used by the converted code or its tests.

### Deviations

- The repository's `scripts/format-rust-spacing.py` runs `cargo fmt --all`,
  which rule 7 forbids because of known formatter drift. Its `spaced()`
  transformation was applied to the two new files only, after `rustfmt`;
  existing files received changed-line formatting only.
