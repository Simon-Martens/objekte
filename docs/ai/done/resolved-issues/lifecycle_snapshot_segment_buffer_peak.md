# Lifecycle snapshot retains encoded segment buffers during save and restore

- **Category:** perf
- **Originating task:** `plan-1-snapshot-persistence-repair`
- **Branch/commit:** `integration/incremental-index-availability` at `2cb74685e113e12dd6b5f34bc36f57781e4ea2d8`
- **Code:** `crates/core/src/storage.rs` (`LifecycleCacheState::from_publication`, `save_lifecycle_snapshot_with_stats`, `read_section`) and `crates/core/src/lifecycle_segment_codec.rs`.

## Observation

The repaired Musenalm snapshot retains 102,881,273 bytes of encoded segment payloads in the lifecycle state before/during outer section serialization. Seven fresh-process profile trials show repaired median construction peak RSS 564.54 MB, build-plus-save peak 715.42 MB, and post-save RSS 604.43 MB. The median full save call is 293.98 ms. Fresh restore peaks at 405.38 MB and ends at 328.26 MB. `read_section` deserializes the entire lifecycle section before segment indexes are reconstructed, so encoded and decoded representations coexist during restore. The paired raw measurements and method are in `bench/results/incremental-index-availability/2026-11-13-snapshot-repair-profile-pairs/`; byte attribution is in the sibling unprofiled results directory.

## Impact and scope

The representation repair removed 46.87 MB from the snapshot and lowered save and restore peaks, but save peak remains about 174 MB above the same-machine original-master nonincremental reference. Comparing process peaks from different workloads does not isolate an allocation, yet the retained 102.88 MB buffer is a concrete contributor. The task explicitly limits this pass to metadata deduplication, derived lookup removal, empty base compaction, and reassessment; a streaming format change would be a separate design and compatibility revision.

## Possible follow-up

A bounded design could encode and checksum each segment directly through the outer snapshot writer and decode validated segments one at a time, avoiding retention of all encoded payloads. It must preserve a coherent pinned publication, section checksums/limits, trailing-byte rejection, atomic replacement, historical metadata references, and complete structural validation before publication. Measure external save call time and fresh restore RSS after that change. This is a proposal for review, not an authorized implementation task.

## Resolution in `remove-legacy-snapshot-stack`

The legacy `OBJSNAP1` snapshot writer and reader, their encoded segment buffers, and `lifecycle_segment_codec.rs` were deleted in `da608b2799`. Managed generations are now the only persistence path, so the reported allocation can no longer occur. This is a historical issue record.
