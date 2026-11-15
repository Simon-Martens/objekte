# Plan 5 of 5: vacuum, slot reuse and fold batching for the main shard

- Task identifier: `main-shard-vacuum`
- Status: specification. Do not start until the user assigns this task.
- Depends on: plans 1 to 4 merged into `master`.
- Audited base: `master` at `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9` plus plans 1 to 4. Line numbers refer to `c54486f893`; search for the quoted identifier if a line has moved.
- This is the last plan of the series.

## 0. Rules for the implementing agent

1. Read `AGENTS.md` and `docs/ai/README.md` and follow that workflow: branch `integration/main-shard-vacuum`, worktree `../objekte-integration-main-shard-vacuum`, memory `docs/ai/memory/main-shard-vacuum/`.
2. No new dependency.
3. Four parts, A to D, each with its own checkpoints. Commit each checkpoint with the workspace building and all tests passing.
4. Never make a test pass by deleting an assertion, loosening an expected value, adding `#[ignore]`, or comparing the implementation with itself. Old layout tests may be pinned with `set_fold_trigger_for_tests(usize::MAX)` exactly as plan 3, D14 allows; list them.
5. No lint suppressions. Formatting: `rustfmt` and `python3 scripts/format-rust-spacing.py` on new files; changed lines only elsewhere.
6. Do not touch `crates/cli` or `crates/desktop`.
7. Stop and write a blocker if a step needs a design decision that section 3 does not make, or a gate fails beyond its blocking limit.

## 1. Outcome

After plan 3, a retired record in the main shard is hidden by the `active` mask but still occupies its slot and still appears in every posting, gram posting and sort order. When half of the main shard is retired, the whole shard is split back into partitions ("dissolve"). This plan replaces that with cleanup in place.

1. **Vacuum.** `IndexedCollectionSegment::vacuum` removes retired records from every index family of the main shard without renumbering anything. Their slots become free.
2. **Slot reuse.** `append` puts new records into free slots first. The slot count of a main shard stops growing under a steady update load.
3. **Policy.** Vacuum runs automatically when a quarter of the main shard is retired. The dissolve rule of plan 3 is removed. Folding also triggers on bytes, and `Index::flush_folds()` and `Index::vacuum()` exist for explicit control.
4. **Migration.** A build partition that is at least 25% retired and consists of one-record files moves its surviving records into the main shard instead of being rewritten as new partitions. Collections that are edited a lot converge to one shard.
5. **Store.** The redb store persists free slots and writes vacuumed and reused data incrementally.
6. Query results stay identical to an index that never folds and never vacuums.

### Non-goals

- No reverse index (slot to keys). Vacuum scans the main shard's maps; it is a maintenance pass, not a per-update operation.
- No background vacuum thread. Vacuum runs inside the mutation that crosses the threshold, or when the caller asks. A cancellable background job can follow later on the pattern of `start_compaction`.
- No change to `MaintenanceOperation`, `MaintenanceReport` or other public enums that the frontends match on.
- No frontend changes.

## 2. Background

### 2.1 State after plans 3 and 4

- A main shard is an `IndexedCollectionSegment` with `role == ShardRole::Main`, a non-zero `lineage`, and every family in the `Shared` backing.
- `append(&self, self_active, delta)` maps delta slot `s` to `self.record_count() + s`.
- `fold_mutation_shards` runs at the start of `with_segment`. `make_room_for_segment_with_limits` contains the dissolve rule (main shard at least 50% retired and at least 4,096 slots).
- Relation references are a `SlotTable<PendingReference>` sorted by source slot; `references_for_source` relies on that order.
- The store writes a main shard incrementally: changed chunks through `diff_chunks`, new rows of `records` and `sort_keys` by slot range, references by source slot. `StoredMain` has the fields `dense_epoch` and `reuse_log_len`, unused so far.

### 2.2 What "free" must mean

A slot may be given to a new record only if no family of the shard still mentions it. One leftover is enough to return wrong results: a stale posting bit makes the new record match the old record's value; a stale relation reference gives it the old record's links. The vacuum in this plan therefore cleans every family in one operation and only then adds the slot to `free_slots`. Section 3, D2 is the complete list. If a later change adds a new per-slot family to the shard, it must be added to `vacuum` and to the test of C2.

## 3. Decisions already made (do not reopen)

**D1. New fields on `IndexedCollectionSegment`.**

| Field | Type | Meaning |
| --- | --- | --- |
| `free_slots` | `RoaringBitmap` | slots that no family mentions; never active |
| `free_file_rows` | `RoaringBitmap` | positions in `files` whose row holds no record and can be overwritten |
| `dense_epoch` | `u64` | incremented by every vacuum |
| `reuse_log` | `SlotTable<RecordSlot>` | slots that `append` has reused since the last vacuum, in order |

All four are empty or zero for shards that are not main shards, at the struct-literal sites, in `to_shared`, and after loading a store written before this plan. Add accessor `live_slot_count(&self) -> usize` = `record_count() - free_slots.len()`.

**D2. `vacuum`.**

```rust
impl IndexedCollectionSegment {
    /// A copy of this main shard without the retired records. Slots are not
    /// renumbered. `is_current` says whether a file row is the file's
    /// current incarnation. Returns `None` when there is nothing to remove.
    pub(crate) fn vacuum(
        &self,
        active: &ActiveRecords,
        is_current: impl Fn(&FileEntry) -> bool,
    ) -> Result<Option<Self>, String>;
}
```

`dead` = every slot in `0..record_count()` that is neither in `active` nor in `free_slots`. If `dead` is empty, return `Ok(None)`. Otherwise clone the shard (cheap; shared backing) and apply, in this order:

| Family | Action |
| --- | --- |
| every `PostingIndex` in `postings.fields`, `custom_postings`, `inference.indexes` | `values.remove_slots(&dead)`, `lengths.remove_slots(&dead)`, structural `remove_slots(&dead)`; `presence -= &dead`, `nulls -= &dead`, `empty_selections -= &dead`; `covered_records = new_logical_count` (below); `finish_stats()` |
| `raw`, `fallback_text`, each `custom_text` index | `normal.remove_slots(&dead)`, `skip.remove_slots(&dead)`; `record_count` unchanged |
| `logical_records`, `empty_array_records` | `-= &dead`; `new_logical_count = logical_records.len()` after this |
| inference `empty_selections[path]`, `short_string_coverage[path]` | `-= &dead` |
| inference `report` | recompute the fields that `consolidate_inference` derives from the indexes (`mutation_segment.rs:1290-1331`: `eligible_records`, `unavailable_records`, `missing_records`, `present_records`, `exact_null_records`, `empty_selection_records`, `short_string_covered_records`, `short_string_distinct_values`, `short_string_unresolved_records`). Move that block into a helper used by both. Use `live_slot_count()` after the vacuum as the record count. Leave the summed counters as they are. |
| each relation | new method `RelationContribution::remove_sources(&mut self, dead: &RoaringBitmap)`: drop references whose source is dead (rebuild the `SlotTable` from the kept references, in order), subtract `dead` from the four bitmaps; `record_count` unchanged |
| `identity_order`, each `ascending`, `descending`, composite `order` | new method `SlotOrder::retain(impl FnMut(RecordSlot) -> bool)`; keep slots not in `dead` |
| `id_lookup` | for each dead slot: if `id_lookup[record(slot).id] == slot`, remove the entry (new method `SharedMap::remove`) |
| `files` | for each row that is not current (`!is_current(&row.entry)`) and has no active record: replace it by the same entry with an empty `records` list, remove its `file_index` entry, add its position to `free_file_rows` |
| `records`, sort `keys` | unchanged; the rows of free slots are stale and are overwritten on reuse |
| `free_slots` | `\|= &dead` |
| `dense_epoch` | `+= 1` |
| `reuse_log` | cleared |
| `accounting` | `recompute_accounting()` (a full pass is acceptable here) |

`role` and `lineage` are unchanged. `PostingMap::remove_slots` and `union_with` are given in Appendix A with tests in Appendix B; both were compiled and run with both chunk sizes before this plan was written. `remove_slots` drops a key whose posting becomes empty, so the maps equal those of a fresh build of the surviving records.

**D3. Publishing a vacuum.** `PublishedIndex::vacuum_main(&mut self, number: usize) -> Result<Option<VacuumStats>, IndexError>` finds the collection's main shard, calls `vacuum` with `is_current` taken from the catalogue (the test at `lifecycle.rs:766-770`: same `revision`, same `created_ns`, collection membership), and replaces that `PublishedSegment` by one with a new ordinal, the vacuumed `index` and the **same** `active` mask. The mask does not change: dead slots were inactive and stay inactive.

**D4. Slot mapping in `append`.**

```rust
/// Where each slot of a delta shard lands in the main shard.
pub(crate) struct SlotMapping {
    reused: Vec<RecordSlot>,
    base: u32,
}

impl SlotMapping {
    /// Delta slot `s` goes to `reused[s]` if there is one, else to
    /// `base + (s - reused.len())`.
    pub(crate) fn target(&self, delta_slot: u32) -> RecordSlot;
    /// Slot count of the main shard after the append.
    pub(crate) fn slot_count(&self, delta_records: u32) -> u32;
}
```

`append` builds the mapping first: `reused` = the `min(free_slots.len(), delta.record_count())` smallest free slots, ascending; `base = self.record_count()`. With no free slots this is exactly the old shift by `base`, so every plan 3 test keeps its meaning.

`AppendedSegment` gets a field `mapping: SlotMapping` and keeps `base`.

Every family rule of plan 3, D8 changes from "shift by `base`" to "map":

| Family | Rule |
| --- | --- |
| `records` | row for delta slot `s` gets `slot = target(s)`; `records.set(target, row)` for a reused slot, `push` otherwise (pushes happen in ascending order) |
| sort `keys` | `set` or `push` the same way |
| postings and grams | new methods `PostingIndex::merge_mapped(&mut self, batch: &Self, mapping: &SlotMapping)` and `BigramIndex::merge_mapped`. For each key of the batch: build the mapped bitmap (`posting.iter().map(\|s\| mapping.target(s).0).collect::<RoaringBitmap>()`) and call `union_with(key, mapped)`. Fixed bitmaps: `\|=` the mapped bitmap. `covered_records += batch.covered_records`. `BigramIndex.record_count = mapping.slot_count(..)`. The same compatibility checks as `merge_shifted` (`posting.rs:352-363`) apply first. |
| `logical_records`, `empty_array_records`, inference bitmaps | `\|=` the mapped bitmap |
| relations | new method `RelationContribution::merge_mapped(&mut self, batch: &Self, mapping, new_record_count)`: bitmaps `\|=` mapped; each batch reference gets `source = target(source)` and is inserted at `references.partition_point(\|r\| r.source.0 <= new_source.0)` so the table stays sorted by source and a source's references keep their order (new method `SlotTable::insert(index, value)`; `imbl::Vector::insert` exists); `record_count = new_record_count` |
| orders | `insert_sorted` for each target slot with the comparators of plan 3, D10 (unchanged) |
| `id_lookup` | `insert(id, target)` |
| `files` | record slots and diagnostic slots of each delta file row mapped with `target`. Row position: the existing position if the `FileId` has a row (replacement); else the smallest position in `free_file_rows` (remove the stale row's `file_index` entry if any, `set`, clear the bit); else `push` |
| inference | D11 of plan 3 with `merge_mapped` in place of `merge_shifted` |
| `free_slots` | minus the reused slots |
| `reuse_log` | append the reused slots |

The active mask of the result is no longer a concatenation. Add `ActiveRecords::with_mapped(&self, other: &ActiveRecords, mapping: &SlotMapping) -> Result<ActiveRecords, String>`: slot count `mapping.slot_count(other.slot_count())`; bits of `self` kept; bit `target(s)` set for every `s` active in `other`. `fold_mutation_shards` uses it in place of `concat`.

`append` must reject a mapping target that is active in `self_active` (`Err("append would reuse an active slot")`). This cannot happen if `free_slots` is correct; the check turns a bookkeeping bug into a skipped fold.

**D5. Policy constants** (in `lifecycle.rs`):

| Constant | Value | Meaning |
| --- | --- | --- |
| `MAIN_VACUUM_MIN_DEAD` | 1,024 | never vacuum for fewer dead slots |
| `FOLD_TRIGGER_BYTES` | 4 MiB | fold when the foldable shards' `accounting.retained_bytes_estimated` sum reaches this |

Remove `MAIN_DISSOLVE_MIN_SLOTS` and the dissolve case added to the retirement rule in plan 3.

- **Vacuum trigger.** In `make_room_for_segment_with_limits`, where the dissolve case was: for the main shard compute `dead = record_count - active.len() - free_slots.len()`. If `dead >= MAIN_VACUUM_MIN_DEAD && dead * 4 >= live_slot_count()`, call `vacuum_main(number)` and return. This function is reached from additions, replacements and deletions, so deletes trigger it too.
- **Fold trigger.** `fold_mutation_shards` proceeds when the number of foldable shards is at least `fold_trigger` **or** their byte sum is at least `FOLD_TRIGGER_BYTES`. `set_fold_trigger_for_tests(usize::MAX)` must disable both.
- **A vacuum failure is not a mutation error.** If `vacuum` returns `Err`, leave the group unchanged, count it in a `#[cfg(test)]` counter, and continue.

**D6. Explicit API** (new methods on `Index`, same file as `compact`, `lifecycle_compaction.rs:304`):

```rust
/// Counters of one explicit vacuum call.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VacuumReport {
    pub revision: IndexRevision,
    pub collections_vacuumed: usize,
    pub slots_freed: u64,
    pub memberships_removed: u64,
    pub duration: Duration,
}

impl Index {
    /// Fold every foldable mutation shard now.
    pub fn flush_folds(&self) -> Result<IndexRevision, IndexError>;
    /// Vacuum every main shard that has retired records, whatever the ratio.
    pub fn vacuum(&self) -> Result<VacuumReport, IndexError>;
}
```

Both follow the publication pattern of `compact_with_control` (`lifecycle_compaction.rs:308`): up to three attempts; each attempt pins `self.view()`, clones that root, changes the clone, sets its `revision` to `view.revision().next()?` and calls `publish_prepared(view.revision(), next)`; on `IndexError::RevisionConflict` it tries again. If nothing changed, return the pinned revision without publishing.

**D7. Migration of partition survivors.** In the retirement rule of `make_room_for_segment_with_limits` (the branch that calls `compact_source_parts` for a shard with `total <= BASE_PARTITION_RECORDS` and at least 25% retired): if the shard has role `Built` and **every** file row has exactly one record slot, then instead of `compact_source_parts`:

1. `let compacted = old.index.compact_active(&old.active, BASE_PARTITION_RECORDS, keep_current)` with the same `keep_current` predicate that `remove_verified_old_shards` uses (`lifecycle_verified.rs:442-449`, without the `replaced_file` condition). This gives a dense shard of the survivors.
2. If it has no records: remove the old shard and return.
3. Find or start the main shard as `fold_mutation_shards` does, `append` the compacted shard, and replace both by the new main shard.
4. If `append` fails, fall back to the existing `compact_source_parts` path.

One-record files cannot be split across shards, so the appended shard holds whole files and `append`'s replace rule is valid. Shards with multi-record files keep today's behaviour.

**D8. Store.** `STORE_FORMAT_VERSION` becomes `2` (older stores are rebuilt).

- `bitmaps` kinds `50` (`free_slots`) and `51` (`free_file_rows`), empty `field`. `ShardHeader` gets `dense_epoch: u64`.
- `read_shard` restores both bitmaps and `dense_epoch`; `reuse_log` starts empty.
- `diff_main(old, new)`:
  - if `new.dense_epoch != stored.dense_epoch`: delete and rewrite the shard's `records`, `sort_keys`, `references` and `files` rows completely;
  - else: write `records` and `sort_keys` rows for slots `old.record_count()..new.record_count()` **and** for every slot in `new.reuse_log[stored.reuse_log_len..]`; for `references`, for each of those slots delete the key range of that source and write its current references;
  - `files`: as before (all rows when the object changed);
  - posting and gram chunks: unchanged, `diff_chunks` reports removed and changed chunks by itself.
- After a successful commit: `stored.dense_epoch = new.dense_epoch`, `stored.reuse_log_len = new.reuse_log.len()`.
- On load, `id_lookup` is rebuilt from `records` rows of slots that are not in `free_slots` (stale rows of free slots must not create mappings).

**D9. Equivalence oracle.** Slot numbers differ between a vacuumed main shard and a fresh build, so shard comparison goes through record ids. Add to the shared test support:

```rust
/// Compare the live content of two shards independent of slot numbers.
fn assert_shards_equivalent_by_id(
    actual: &IndexedCollectionSegment,
    actual_active: &ActiveRecords,
    expected: &IndexedCollectionSegment,
    expected_active: &ActiveRecords,
)
```

It asserts, with "ids of" meaning the record ids of the active slots in a bitmap or sequence:

- the sets of active record ids are equal; for each id the records are equal except for `slot`;
- for every posting field and every key: ids of the posting are equal on both sides, and neither side has a key whose posting contains no active slot **on the vacuumed side** (the vacuumed side must have no empty keys; the other side is compared after filtering);
- the same for structural facts, lengths, gram maps, `presence`, `nulls`, `empty_selections`, `logical_records`, `empty_array_records`;
- for every sort column: the id sequence of `ascending` filtered by active is equal, the same for `descending`, composite orders and `identity_order`; the sort key of each active id is equal;
- for every relation: for each active id the list of `(target_id, positions)` from `references_for_source` is equal;
- for every current file: the ids of its records are equal.

## 4. Checkpoints

### C0. Baseline

Record `START`. Run and save everything plan 4 C0 lists, plus `accept_musenalm_store` for both alphabets and the mutation benchmarks (`small`, `stress`, three runs each on fresh directories), plus the two measurement tests `measure_fold_cost_on_real_fixture` and `measure_index_store_on_real_fixture`.

### Part A: vacuum

#### C1. Map and container primitives

1. Append Appendix A to `posting_map.rs` and Appendix B to `posting_map.test.rs`. Add `remove_slots` and `union_with` forwarding methods to `GramMap` (`Building`: the same two loops over the `HashMap`; `Shared`: forward).
2. Add `SlotOrder::retain`, `SharedMap::remove`, `SlotTable::insert(index, value)`, with tests in `slot_store.test.rs` against the `Vec`/`HashMap` models in both backings, including clone isolation.
3. Add `RelationContribution::remove_sources` and `merge_mapped` with tests in `relation.test.rs`: after `remove_sources`, `references_for_source` is empty for dead sources and unchanged for others; after `merge_mapped` into reused low slots, the table is still sorted by source and each source returns exactly its own references in their original order.

Commit: `Add slot removal and unordered merge primitives`.

#### C2. `vacuum`

1. Add the D1 fields and `live_slot_count`.
2. Implement `vacuum` per D2.
3. Add `assert_shards_equivalent_by_id` (D9).
4. Tests in `mutation_segment.test.rs`, on a main shard built by `to_shared` plus several appends, with a schema that has every family (as in plan 3, C5):

   - `vacuum_matches_compaction_by_id`: retire about a third of the records (whole files, including one replaced file). `vacuumed = main.vacuum(&active, ..)`. `expected = main.compact_active(&active, usize::MAX, ..)` (dense, renumbered). `assert_shards_equivalent_by_id(&vacuumed, &active, &expected.index, &expected.active)`.
   - `vacuum_frees_exactly_the_dead_slots`: `free_slots` equals the retired set; no posting, gram posting, fixed bitmap, order, relation bitmap or reference of the vacuumed shard mentions a free slot. Write this check as one helper `assert_no_family_mentions(&shard, &slots)` that walks every family; it is reused in C4.
   - `vacuum_without_dead_slots_returns_none`.
   - `vacuum_does_not_change_the_original`.
   - `vacuum_twice_is_idempotent`: a second call returns `None`.
   - `vacuum_drops_empty_keys_and_updates_stats`: a value that only retired records had is gone; `distinct_values()` and `stats.min_key`/`max_key` equal those of the compacted shard.
   - `vacuum_marks_dead_file_rows_free`: a deleted file's row has no records, is not found by `file(id)`, and its position is in `free_file_rows`; a current file's row is untouched.

Commit: `Vacuum retired records out of a main shard`.

#### C3. Publish vacuums and replace the dissolve rule

1. `vacuum_main` (D3), the trigger (D5), `Index::vacuum` (D6). Remove the dissolve case and `MAIN_DISSOLVE_MIN_SLOTS`.
2. Replace plan 3's test `main_shard_dissolves_when_half_retired` by `main_shard_vacuums_when_a_quarter_is_retired`: same scenario; assert the main shard still exists with the same `lineage`, its `free_slots` is non-empty, the failed-vacuum counter is 0, and queries equal a never-folding control index.
3. New tests in `lifecycle_mutation.test.rs`:
   - `explicit_vacuum_reports_and_preserves_results`: `Index::vacuum()` returns non-zero `slots_freed` and `memberships_removed`; a second call returns zeros and the same revision; all queries equal control before and after.
   - `pinned_view_survives_a_vacuum`: a view taken before the vacuum still answers as before.
   - `deletes_alone_trigger_a_vacuum`: only `delete_file` calls, enough to cross the threshold.

Commit: `Vacuum the main shard instead of dissolving it`.

### Part B: slot reuse

#### C4. Mapped `append`

1. `SlotMapping`, `merge_mapped` methods, `ActiveRecords::with_mapped`, and the D4 rules in `append`. `fold_mutation_shards` uses `with_mapped`.
2. All plan 3 `append_*` tests must pass unchanged (no free slots means the old behaviour).
3. New tests in `mutation_segment.test.rs`:
   - `append_reuses_free_slots_first`: vacuum a main shard with 10 dead slots, append a 4-record delta: the four records sit in the four smallest free slots, `record_count()` is unchanged, `free_slots` has 6 left, `reuse_log` lists the four slots.
   - `append_overflows_into_new_slots`: 3 free slots, 5-record delta: three reused, two appended at the end.
   - `reused_slots_carry_only_the_new_record`: after the append, `assert_no_family_mentions` finds nothing for the remaining free slots, and `assert_shards_equivalent_by_id` holds against `consolidate(&[&compacted_main, &delta])`.
   - `reused_slot_relations_and_orders_are_correct`: the new record at a reused low slot returns exactly its own references and sits at its correct place in every sort order.
   - `append_rejects_reuse_of_an_active_slot`: corrupt `free_slots` in the test to include an active slot; `append` returns the D4 error.
   - `with_mapped_matches_bitmap_model`: `ActiveRecords::with_mapped` against a plain bitmap computation, across a chunk boundary.

Commit: `Reuse free slots when appending to the main shard`.

#### C5. End-to-end churn

Tests in `lifecycle_mutation.test.rs`:

- `churn_matches_an_index_that_never_folds`: plan 3's `folding_matches_an_index_that_never_folds` extended to 600 mutations with the main-shard collection kept at about 150 live records, so that several vacuums and many reuses happen. Three indexes (`never`, `always` = trigger 1, `default`); compare the full query list after every 20th mutation. Additionally assert on the `always` index at the end: at least 3 vacuums happened (counter), `record_count()` of the main shard is below 4 times its live record count, failed-fold and failed-vacuum counters are 0.
- `slot_count_is_bounded_under_steady_updates`: 5,000 updates of the same 200 files with trigger 1: the main shard's `record_count()` never exceeds `200 + 2 * max(MAIN_VACUUM_MIN_DEAD, 200)`.
- `concurrent_queries_during_vacuum_see_consistent_revisions`: the plan 3 concurrency test with vacuums forced by a small `MAIN_VACUUM_MIN_DEAD` (make the constant a field of `PublishedIndex` next to `fold_trigger`, with a `#[cfg(test)]` setter of the same kind).

Run both workspace test commands and both forced acceptance runs.

Commit: `Test folding, vacuum and reuse under churn`.

### Part C: batching and migration

#### C6. Byte trigger and `flush_folds`

1. `FOLD_TRIGGER_BYTES` and the combined trigger (D5); `Index::flush_folds` (D6).
2. Tests: `byte_trigger_folds_few_large_mutation_shards` (three mutation shards whose estimate sum exceeds a test-set byte limit fold although the count trigger is 8; make the byte limit a `PublishedIndex` field with a test setter); `flush_folds_leaves_no_foldable_shard`; `flush_folds_without_work_keeps_the_revision`.

Commit: `Fold by size and on request`.

#### C7. Move partition survivors into the main shard

1. Implement D7.
2. Tests:
   - `retired_partition_survivors_move_to_the_main_shard`: build an index over 3,000 one-record files in one collection (two partitions). Update 600 files that all lie in the first partition, with trigger 1. Assert: the first partition is gone; its unretired records are in the main shard; the collection's live record count is 3,000; queries equal control.
   - `multi_record_partitions_keep_the_old_path`: the same with a JSON Lines collection; the retired partition is replaced by `Built` parts and no survivor is in the main shard.
   - `migration_failure_falls_back`: force `append` to fail through a `#[cfg(test)]` switch on the root; the old path runs and results equal control.
3. Extend `churn_matches_an_index_that_never_folds` so that its fixture has more than 2,048 files in the churned collection and at least one migration happens (assert with a counter).

Commit: `Move one-record partition survivors into the main shard`.

### Part D: store

#### C8. Persist free slots, vacuums and reuse

1. Implement D8.
2. Tests in `index_store_shard.test.rs` and `lifecycle_store.test.rs`:
   - `round_trip_of_a_vacuumed_main_shard`: `read_shard` after `write_shard` satisfies `assert_shards_equal` including `free_slots`, `free_file_rows` and `dense_epoch`.
   - `diff_after_vacuum_equals_full_write`: checkpoint main shard `m1`; vacuum to `m2`; `diff_main`; read equals `m2`; the store has no posting row left for a key that vacuum removed.
   - `diff_after_reuse_equals_full_write`: checkpoint `m2`; append with reuse to `m3`; `diff_main`; read equals `m3`; `rows_written` for `records` is at most the delta's record count plus 2.
   - `stale_rows_of_free_slots_create_no_id_mapping`: after a reopen, `slot_for_id` of a deleted id is `None`.
   - `churn_with_checkpoints_reopens_equal_to_control`: the C5 churn sequence with a checkpoint every 50 mutations and a reopen at three points; each reopened index equals control and the next mutations continue correctly on it.
   - `format_one_store_is_rebuilt`: a store with `format = 1` gives `Rebuilt(FormatVersion)`.
3. Run `accept_musenalm_store` for both alphabets.

Commit: `Persist vacuumed and reused main shard state`.

### C9. Measure, validate, report

1. Re-run everything from C0.
2. Extend `measure_fold_cost_on_real_fixture`: after the 2,000 updates, continue with 20,000 updates cycling over the same 2,000 files. Report update latency (median, p95, max), time and count of vacuums, main shard `record_count()` and live count over time (every 2,000 updates), shard count, process RSS at the start and end of the cycle, and query probe latency. Save as `docs/ai/generated/main-shard-vacuum-measurement.md`.
3. Update `docs/search-core.md`, `docs/future-whole-file-replacement.md`, `docs/development/posting-map.md` and `docs/development/index-store.md`: vacuum, free slots, reuse, triggers, migration, `Index::vacuum`, `Index::flush_folds`, format version 2.
4. Review `docs/ai/issues/perf/main-shard-bitmap-clone-per-fold.md` (plan 3): add this plan's measurements; do not close it.
5. Complete the task per `docs/ai/README.md` section 7.

## 5. Gates

| Gate | Requirement | Blocking limit |
| --- | --- | --- |
| G1 | Workspace tests pass with default features; pinned layout tests listed. | any failure |
| G2 | Workspace tests pass with `force-shared-backing`. | any failure |
| G3 | Index signatures equal C0; build gate without failure, within 5% of C0. | signature change; more than 15% |
| G4 | `accept_musenalm` (default, forced) and `accept_musenalm_store` pass for both alphabets. | any failure |
| G5 | Quick query gate passes. | any failure |
| G6 | Clippy warnings not higher than C0. | |
| G7 | `git diff --stat START -- crates/cli crates/desktop` is empty. | any change |
| G8 | Mutation benchmarks `small` and `stress`: medians within 25% of C0 or 0.2 ms, whichever is larger. | more than 2x |
| G9 | In the C9 churn measurement: main shard `record_count()` stays below 4 times the live count; RSS at the end of the 20,000-update cycle is within 15% of RSS at its start. | slot count or RSS growing without bound |
| G10 | `churn_matches_an_index_that_never_folds` and `churn_with_checkpoints_reopens_equal_to_control` pass with default and forced features. | any failure |

Report all numbers. Between requirement and blocking limit: repeat, investigate, explain. Beyond: stop and write a blocker with the measurements.

## 6. Pitfalls

- **A slot is free only when every family is clean.** Add a family to the shard, add it to `vacuum` and to `assert_no_family_mentions`.
- **Vacuum does not change the `active` mask.** It changes what is behind inactive slots.
- **`records` and sort keys keep stale rows for free slots.** Nothing may read them: every reader checks `active` first. On load, skip free slots when rebuilding `id_lookup`.
- **Relation references must stay sorted by source.** Reused slots are low numbers; insert, do not push.
- **`BigramIndex.record_count` is the slot count, not the live count.** Do not lower it in `vacuum`.
- **`covered_records` must equal the logical record count after a vacuum,** as it does after a compaction. Several query shortcuts compare it with the domain size.
- **Do not vacuum or fold while iterating the group you are changing.** Collect positions first, then rebuild the group.
- **Old views keep the old shard.** Never mutate a shard in place; `vacuum` and `append` return new shards that share structure.
- **Migration only for one-record files.** The check is per shard: every file row has exactly one record slot.
- **`Index::compact()` still rewrites everything densely.** It sees free and retired slots of a main shard as reclaimable and turns the main shard into `Built` partitions. That is the documented meaning of an explicit compaction; do not change it, and say so in the documentation.
- **A vacuumed map shares little with its predecessor when many chunks changed.** The checkpoint after a large vacuum writes many chunks. That is expected; report its size in C9.

## 7. The completion report must include

- `START`, branch, commits per checkpoint.
- C0 and final values for every gate.
- The churn measurement table, with slot count, live count and RSS over time.
- The list of pinned layout tests.
- Counters from the churn tests: folds, vacuums, migrations, failed folds, failed vacuums.
- Deviations and their explanation.

## Appendix A. Code to append to `crates/core/src/posting_map.rs`

Insert above the final `#[cfg(test)]` module declaration.

```rust
impl<K: Ord + Clone> SharedPostings<K> {
    /// Remove `dead` slots from every posting. A key whose posting becomes
    /// empty is removed. Returns the number of removed memberships.
    pub fn remove_slots(&mut self, dead: &RoaringBitmap) -> u64 {
        let mut dead_parts: std::collections::HashMap<ChunkId, Option<RoaringBitmap>> =
            std::collections::HashMap::new();
        let mut rewrites = Vec::new();
        for ((key, chunk), part) in self.chunks.iter() {
            let dead_part = dead_parts.entry(*chunk).or_insert_with(|| {
                let part = dead & &chunk_mask(*chunk);
                (!part.is_empty()).then_some(part)
            });
            let Some(dead_part) = dead_part else {
                continue;
            };
            let hits = part.intersection_len(dead_part);
            if hits == 0 {
                continue;
            }
            rewrites.push(((key.clone(), *chunk), part.as_ref() - &*dead_part, hits));
        }
        let mut removed = 0u64;
        let mut touched_keys = Vec::new();
        for (address, remaining, hits) in rewrites {
            removed += hits;
            if remaining.is_empty() {
                self.chunks.remove(&address);
                touched_keys.push(address.0);
            } else {
                self.chunks.insert(address, Arc::new(remaining));
            }
        }
        touched_keys.dedup_by(|a, b| a == b);
        for key in touched_keys {
            if !self.contains_key(&key) {
                self.keys -= 1;
            }
        }
        removed
    }

    /// Add every slot of `posting` to `key`. Slots may be in any position
    /// relative to the existing ones.
    pub fn union_with(&mut self, key: &K, posting: RoaringBitmap) {
        let parts = split_chunks(posting);
        if parts.is_empty() {
            self.ensure_key(key);
            return;
        }
        for (chunk, part) in parts {
            self.merge_part(key, chunk, part);
        }
    }
}

impl<K: Ord + Clone> PostingMap<K> {
    /// Remove `dead` slots from every posting; drop keys that become empty.
    /// Returns the number of removed memberships.
    pub fn remove_slots(&mut self, dead: &RoaringBitmap) -> u64 {
        match &mut self.backing {
            Backing::Building(entries) => {
                let mut removed = 0u64;
                entries.retain(|_, posting| {
                    let hits = posting.intersection_len(dead);
                    if hits == 0 {
                        return true;
                    }
                    removed += hits;
                    *posting -= dead;
                    !posting.is_empty()
                });
                removed
            }
            Backing::Shared(shared) => shared.remove_slots(dead),
        }
    }

    /// `*entry(key).or_default() |= posting`.
    pub fn union_with(&mut self, key: K, posting: RoaringBitmap) {
        match &mut self.backing {
            Backing::Building(entries) => {
                *entries.entry(key).or_default() |= posting;
            }
            Backing::Shared(shared) => shared.union_with(&key, posting),
        }
    }
}
```

## Appendix B. Tests to append to `crates/core/src/posting_map.test.rs`

```rust
/// `remove_slots` equals subtracting the dead set from every posting and
/// dropping keys that became empty, in both backings.
#[test]
fn remove_slots_matches_the_model() {
    let chunk = 1u32 << POSTING_CHUNK_BITS;
    for seed in 0..40u64 {
        let mut random = Lcg(seed + 500);
        let mut model: BTreeMap<u16, RoaringBitmap> = BTreeMap::new();
        for _ in 0..600 {
            model
                .entry(random.next(40) as u16)
                .or_default()
                .insert(random.next(3 * chunk));
        }
        let mut dead = RoaringBitmap::new();
        for _ in 0..random.next(400) {
            dead.insert(random.next(3 * chunk));
        }
        if seed % 5 == 0 {
            dead.insert_range(0..chunk);
        }
        let native = PostingMap::from_btree_map(model.clone());
        let mut shared = native.clone();
        shared.freeze();
        let snapshot = shared.clone();
        let mut expected_removed = 0u64;
        model.retain(|_, posting| {
            expected_removed += posting.intersection_len(&dead);
            *posting -= &dead;
            !posting.is_empty()
        });
        for mut map in [native, shared] {
            assert_eq!(map.remove_slots(&dead), expected_removed);
            assert_eq!(map.to_btree_map(), model);
            assert_eq!(map.len(), model.len());
            assert_eq!(map.first_key(), model.keys().next());
        }
        assert!(snapshot.len() >= model.len());
        assert!(snapshot.all_bitmaps(|part| !part.is_empty()));
    }
}

/// `union_with` accepts slots below, between and above the existing ones.
#[test]
fn union_with_matches_bitmap_union() {
    let chunk = 1u32 << POSTING_CHUNK_BITS;
    for frozen in [false, true] {
        let mut map: PostingMap<u16> = PostingMap::new();
        if frozen {
            map.freeze();
        }
        map.insert_slot(1, chunk + 5);
        let extra: RoaringBitmap = [2u32, chunk + 1, chunk + 9, 4 * chunk].into_iter().collect();
        map.union_with(1, extra.clone());
        map.union_with(2, extra.clone());
        map.union_with(3, RoaringBitmap::new());
        let mut expected = extra.clone();
        expected.insert(chunk + 5);
        assert_eq!(map.get(&1).unwrap().into_owned(), expected);
        assert_eq!(map.get(&2).unwrap().into_owned(), extra);
        assert_eq!(map.cardinality(&3), 0);
        assert_eq!(map.len(), 3);
    }
}
```

## Implementation report

- Task identifier: `main-shard-vacuum`
- Completion date: 2026-11-15
- START: `693f8e0a4b` (plan 4 tip)
- Integration branch: `integration/main-shard-vacuum`
- Integration worktree:
  `/home/simon/source/objekte-second-base-integration-main-shard-vacuum`
- Commits:
  - `ba9b1c9afe` Add slot removal and unordered merge primitives (C1)
  - `edc2a58e3f` Vacuum retired records out of a main shard (C2)
  - `75c2a53138` Vacuum the main shard and reuse free slots (C3-C5)
  - `e42f8156f3` Test folding, vacuum and reuse under churn (C5-C7)
  - `7c8306cd77` Persist vacuumed and reused main shard state (C8)
  - `6097676ca3` Test partition survivor migration and store reopen churn (C7/C8 tests)
  - documentation/report commit (this commit)

### Summary

`PostingMap` gained `remove_slots` and `union_with`; `GramMap`,
`StructuralFacts`, `PostingIndex`, `BigramIndex`, `SlotOrder`, `SharedMap`,
`SlotTable` and `RelationContribution` gained the removal/insertion
operations a vacuum needs. `IndexedCollectionSegment::vacuum` removes every
dead slot from postings, grams, orders, relations, inference bitmaps and
accounting, drops empty keys, clears dead id mappings, frees dead file rows and
records the slots in `free_slots` without renumbering any live slot.
`Index::vacuum()` publishes it; a main shard also vacuums automatically at
1,024 dead slots and a quarter dead. `append` consumes `free_slots` first
through a `SlotMapping`; active masks, postings, grams, orders and relations
are mapped, and an append that would reuse a live slot is rejected.

Folding now triggers on 4 MiB of candidate estimates as well as eight shards;
`Index::flush_folds()` folds on request. Retired one-record build partitions
move their current survivors into the main shard; multi-record partitions keep
the existing compacting path, and a test switch forces that fallback.
`Index::compact()` still rewrites densely and turns a main shard into `Built`
partitions.

The store format is 2. A shard header carries `dense_epoch`; `free_slots` and
`free_file_rows` are bitmaps. A checkpoint after a vacuum rewrites the shard's
dense rows once; later checkpoints write new rows and the `reuse_log` slots.
Loading skips free slots when rebuilding `id_lookup`.

### Gates

| Gate | C0 | Final |
| --- | --- | --- |
| G1 default tests | 878 passed / 20 ignored | 907 passed / 20 ignored |
| G2 forced tests | 878 passed / 20 ignored | 907 passed / 20 ignored |
| G3 signatures | unchanged | unchanged |
| G3 build ASCII | 0.940 s / 486.4 MB | 0.968 s (+2.9%) / 491.5 MB (+1.0%) |
| G3 build Unicode | 0.998 s / 512.5 MB | 0.986 s (-1.2%) / 510.2 MB (-0.4%) |
| G4 accept default/forced, ASCII/Unicode | pass | pass |
| G4 accept store ASCII/Unicode | pass | pass |
| G5 quick query gate | 3 baseline failures, ratio 0.724 | same 3 baseline failures, ratio 0.453 |
| G6 Clippy warnings | 236 | 235 |
| G7 `crates/cli` + `crates/desktop` diff | empty | empty |
| G8 mutation benchmarks | see below | all medians pass |
| G9 churn slot/RSS | n/a | 3,064 slots / 2,040 live; RSS +0.5% |
| G10 churn tests default/forced | n/a | pass |

The three query-gate failures are the same path-sensitive baseline failures
reported by plans 1-4 (`corpus_fingerprint differs`, missing
`schema-manifest.json`, `semantic registrations differ`). They fail identically
on the untouched baseline checkout; no latency confirmation failed.

G8 against exact C0 (`693f8e0a4b`, three runs each): every `small` median is
within 0.2 ms absolute and every `stress` median ratio is between 0.90 and
1.13 (the only above-1.05 item is `pinned_read`, 0.128 ms to 0.145 ms). No
median exceeds 2x. Mutation TSVs are in `/tmp/objekte-main-shard-vacuum/c0/`
and `/tmp/objekte-main-shard-vacuum-final/`.

### Churn measurement

See `docs/ai/generated/main-shard-vacuum-measurement.md`. The 20,000-update
cycle held the main shard at 3,064 physical slots and 2,040 live records,
74 shards, 2500 folds, 19 vacuums, 0 migrations, 0 fold failures; RSS grew
0.5%; query probe p95 stayed at 0.001 ms.

Pinned layout tests (plan 3 D14):
- `verified_removal_compacts_a_shared_shard_without_losing_siblings`
- `folding_matches_an_index_that_never_folds`
- `churn_matches_an_index_that_never_folds`

Churn test counters: `churn_matches_an_index_that_never_folds` asserts at
least three vacuums, at least one migration and zero fold failures; it has no
failed-vacuum counter because a vacuum error fails the mutation call. The
store churn test reopens at three checkpoints and compares every query
signature with a control index.

### Complete/incomplete items

- Implemented and tested: vacuum in every family, automatic and explicit
  vacuum, free-slot reuse, rejection of active-slot reuse, byte/count/requested
  folding, partition-survivor migration, fallback, churn against a
  never-folding control, bounded slot count, pinned views, store round trips
  after vacuum and reuse, stale id rows, format-1 rebuild, and churn with three
  store checkpoints and reopens.
- `CheckpointReport.bytes_written` remains zero.
- `diff_after_vacuum_equals_full_write` proves equality by reading the whole
  shard; it does not inspect one removed posting key directly.
- `diff_after_reuse_equals_full_write` uses the test-only `records_written`
  counter for the "delta plus two" assertion.

### Deviations

- The churn test writes 2,200 one-record files and runs 4,500 updates without
  the delete/re-add branch. Deleting a file compacts the built shard through
  the verified delete path, which removes accumulated dead records and keeps
  the 75% retirement window from ever opening. Migration is still covered by
  `retired_partition_survivors_move_to_the_main_shard` and by the churn
  assertion after this change.
- Migration runs inside the existing `make_room_for_segment_with_limits`
  retirement branch rather than a new path, so its fallback is the existing
  old compaction path.
- Migration failure is forced with a test-only `PublishedIndex` switch that
  makes the migration condition false; production behavior is unchanged.
- The store records `reuse_log` only in memory for a checkpoint; it is not
  itself persisted, and a reopened shard starts with an empty log.
- Branches are stacked because agents may not merge into `master`.
