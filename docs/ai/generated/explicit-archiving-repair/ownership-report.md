# Ownership and lifecycle evidence

- Native posting directories retain one insertion-assigned `BTreeMap<StoredKey, u32>` and one canonical shared `Vec<RoaringBitmap>`.
- Segment row serialization uses borrowed file, record, diagnostic, path, and scalar adapters. It does not stage owned row vectors.
- Native record-ID publication retains the original `HashMap<RecordId, RecordSlot>` and uses borrowed `str` lookup.
- Archived validation uses borrowed IDs, one visitation bitmap for exact file ownership, and direct hash-map probes.
- The inventory file-row vector is shared. Explicit archive replaces the inventory path wrapper with the converted base path directory.
- Segment conversion jobs deduplicate shared segment owners and are bounded by both requested workers and useful jobs.
- Operation-scoped failure controls are shared across coordinator and worker threads. Failures after successful components leave the public root, generation, revision, owners, and selective query results unchanged.
- Pinned views retain their old native owners. Fresh views use archived owners. Replaced owners are released after the last old view and operation-held references drop.
- Query metadata filters use borrowed record rows and direct scalar file access. A cold zero-result metadata test initializes no whole segment or catalog compatibility cache.
