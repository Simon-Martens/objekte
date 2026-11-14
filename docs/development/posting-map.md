# Posting maps

`PostingIndex` and `StructuralFacts` store their record-slot postings in
`PostingMap<K>` (`crates/core/src/posting_map.rs`). Nothing outside the
defining modules touches the maps directly. This keeps the storage
representation replaceable.

## Backings

- `Building` is the native `BTreeMap<K, RoaringBitmap>` used while an index is
  built and by small immutable shards. It has the lowest per-operation cost.
- `Shared` is a persistent (copy-on-write) `imbl::OrdMap<(K, ChunkId), Arc<RoaringBitmap>>`.
  Each value covers one fixed slot range ("chunk") and keeps absolute slot
  numbers. A clone shares every node, and a write copies only the changed path
  and the one changed chunk.

A chunk holds 65,536 slots (`POSTING_CHUNK_BITS = 16`) in normal builds and
256 slots when the `force-shared-backing` feature is enabled. The small forced
size makes ordinary test shards span several chunks, so the multi-chunk read
and write paths run in the whole test suite. A key whose whole posting is empty
keeps a single empty `(key, 0)` entry, so the key survives a freeze exactly as
an empty bitmap survives in a `BTreeMap`. Reading a key that spans several
chunks assembles a `Cow::Owned` union; a key inside one chunk is borrowed.

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
Sealed shards call `freeze_if_large`, which freezes the posting values, lengths,
structural facts and both gram maps. An already-frozen map is unchanged by a
second `freeze`.

## Gram maps

`BigramIndex` stores its two gram dictionaries in `GramMap`
(`crates/core/src/gram_map.rs`). `Building` is the construction-time
`HashMap<u64, RoaringBitmap>`; `Shared` wraps a frozen `PostingMap<u64>`, so
there is only one chunking implementation. Gram iteration used by signatures
and tests goes through `for_each_sorted`, which sorts the native hash-map keys
and walks the shared map in key order.

## Invariants

- Keys keep their typed `Ord`; `ScalarKey` and `Number` compare numerically
  across representations.
- An empty posting stays stored under its key, exactly as an empty bitmap
  stays in a `BTreeMap`.
- No caller holds a `Cow` across a mutation of the same map.
- Accounting expressions are rebuilt from `value_bytes`, `length_bytes`,
  `distinct_values`, `distinct_lengths`, `key_count`, `estimated_bytes` and
  `value_key_text_bytes`. For the native backing these produce exactly the old
  numbers; for the shared backing they are the sum over stored chunks and may
  differ slightly because each chunk carries its own roaring header.
