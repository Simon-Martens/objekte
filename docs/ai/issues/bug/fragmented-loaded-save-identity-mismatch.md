# Repeated fragmented-store mutation fails sealed-segment identity checks

- Category: bug
- Originating task: `toml-index-manifest`
- Baseline where reproduced: `a0a0691897c6ff24985fc4db9ae51f5d77a89899`
- Candidate where reproduced: `b31db7b2eb97bfe3d5cf9e047a823845b299d647`
- Relevant code: `crates/core/src/hybrid_segment_codec.rs` (`write_sealed_segment`, integrity comparison), `crates/core/src/lifecycle_storage.rs` (`flush_generation`), lifecycle mutation/partition re-sealing.

## Observed behavior

After opening a saved 71-segment Musenalm store and completing two cycles of "update 1,500 source files, reconcile, save", a third cycle that updates ten files and saves fails. The candidate reports:

```text
sealed segment identity changed before persistence:
id_collection=CollectionId(3) segment_collection=CollectionId(3)
metadata_collection=CollectionId(3) id_ordinal=5821 ordinal=5947
metadata_ordinal=5947 id_slots=47 records=48 metadata_slots=48
```

The baseline at `a0a0691897` fails in the same scenario through related persistence invariants:

```text
segment metadata reference order changed
sealed segment identity changed before persistence
```

The failure aborts before manifest publication, so the previous generation remains loadable.

## Expected behavior

A successful reconciliation of changed source files followed by an explicit save should publish a complete new generation, or return a typed mutation/reconciliation error before save. A published `IndexedCollectionSegment` should never contain a sealed reader whose integrity section names a different ordinal or slot count than the segment's current records.

## Reproduction outline

1. Build a saved Musenalm store from a disposable source copy.
2. Load the store, update 1,500 `data/items` files by appending one space, run one reconciliation, and save.
3. Repeat the update/reconcile/save cycle once more.
4. Update ten files, reconcile, and save again.

The temporary benchmark helper used for this reproduction was not committed. The stores and raw run output are under the candidate worktree's ignored `target/toml-index-manifest/bench/` directory.

## Impact and why it was not resolved here

The TOML manifest task could measure fragmented-store loads and bytes written for the first two changed generations, but could not measure a third changed-save time. The failure reproduces on the unmodified baseline and is therefore a pre-existing mutation/partition re-sealing defect rather than a consequence of the manifest, segment-header or catalog changes. Fixing it requires auditing how mutated partitions replace records/ordinals while retaining a sealed reader, which is outside this task's persistence-format scope.

## Possible approaches

Evidence: the sealed reader's integrity ordinal and slot count lag the `IndexedCollectionSegment` fields by one prior state. Hypotheses:

- a partition clone replaces `records`/`ordinal` without re-sealing its reader;
- a mutation path reuses a previous sealed segment object while appending or reassigning slots;
- partition compaction publishes a segment whose reader belongs to an earlier partition incarnation.

A targeted fix should either re-seal the affected segment whenever records, ordinal or slot count changes, or reject the inconsistent segment earlier with a typed mutation error. Add a regression test that performs several update/reconcile/save cycles on a multi-segment loaded store before the third save.
