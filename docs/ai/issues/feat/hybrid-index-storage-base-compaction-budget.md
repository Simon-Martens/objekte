# Bound compaction of base-containing indexes

- **Category:** feat
- **Originating tasks:** `hybrid-index-storage` and its repair specification
- **Observed revision:** `74cab22411` on `integration/hybrid-index-storage`
- **Code:** `crates/core/src/lifecycle_compaction.rs`, especially `base_as_segment` and the base-containing branch of `compact_published_with_budget`

## Observation and expected behavior

The common partitioned-root path compacts a bounded group of shards and reports charged records, input bytes and estimated temporary bytes. When any collection still has records in its original base, `compact_published_with_budget` takes its older branch. That branch clones the base into a segment, processes every collection and retired segment, and returns `CompactionWork::default()`; it does not apply the per-call record, byte, or elapsed budgets to that work. Cancellation checks remain, and publication is atomic, but the call can require collection-sized work while reporting zero charged work.

The repair specification requires maintenance to be resumable or bounded across the supported index shapes, including original-base records. A single compaction call should report the work it actually charged and leave `remaining_retired_slots` when more batches are needed.

## Reproduction and impact

Build or restore an index whose `collection.generation.records` is nonempty, retire a base record, then call `Index::compact()`. The branch condition and returned `CompactionWork::default()` are visible in the code above. The existing partitioned-root stress and locality measurements in [the allocation follow-up report](../../generated/hybrid-index-allocation-query-performance-20261114.md) do not establish a bound for this path. A large legacy base can produce a long maintenance pause and a larger temporary allocation than the normal batch budget suggests. This is a scope gap, not an observed incorrect query result.

## Why deferred and possible approaches

The bounded shard path was implemented and tested during the hybrid work. Converting the base-containing branch requires a publication strategy for partial base retirement while preserving pinned views and exact index facts. That design was not completed before the user's instruction to close the task and track remaining work as issues.

A possible approach is to partition the base into independently replaceable contributions and reuse the existing shard-budget loop. Verify update and compaction semantics, actual work accounting, cancellation, and pinned readers before deleting the older branch. This issue is for review; it does not authorize implementation.
