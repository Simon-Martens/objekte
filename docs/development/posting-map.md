# Posting maps

`PostingIndex` and `StructuralFacts` store their record-slot postings in
`PostingMap<K>` (`crates/core/src/posting_map.rs`). Nothing outside the
defining modules touches the maps directly. This keeps the storage
representation replaceable.

## Backings

- `Building` is the native `BTreeMap<K, RoaringBitmap>` used while an index is
  built and by small immutable shards. It has the lowest per-operation cost.
- `Shared` is a persistent (copy-on-write) `imbl::OrdMap<K, Arc<RoaringBitmap>>`.
  A clone shares every node, and a write copies only the changed path and the
  changed bitmap.

Both backings behave identically for reads, writes, iteration and error
messages. `PostingMap::get` returns `Cow<'_, RoaringBitmap>`, so a call site
must use `as_ref()`/`as_deref()` when it needs `&RoaringBitmap`.

Iteration is forward-only. `range(lower, upper, descending)` and `iter()`
return `PostingIter`; there is no `.rev()`. Range bounds are `Bound<&K>`, and
key order is the key type's `Ord`, never a byte encoding.

## Backing policy

`shared_backing_wanted(record_count)` is the only place that chooses a
backing. It returns true when

- the shard has at least `SHARED_BACKING_MIN_RECORDS` (16,384) records, or
- the crate feature `force-shared-backing` is enabled.

The feature exists for validation: it forces every sealed shard onto the
persistent backing so the whole test suite exercises both representations.
Sealed shards call `freeze_if_large`, which freezes the posting families that
support the shared backing. An already-frozen map is unchanged by a second
`freeze`.

## Invariants

- Keys keep their typed `Ord`; `ScalarKey` and `Number` compare numerically
  across representations.
- An empty posting stays stored under its key, exactly as an empty bitmap
  stays in a `BTreeMap`.
- No caller holds a `Cow` across a mutation of the same map.
- Accounting expressions are rebuilt from `value_bytes`, `length_bytes`,
  `distinct_values`, `distinct_lengths`, `key_count`, `estimated_bytes` and
  `value_key_text_bytes`; they must produce the same numbers as before.
