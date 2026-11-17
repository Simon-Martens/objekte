# Component coverage inventory

This inventory is the phase-1 classification ledger. It is updated as each
component gains its stored descriptor and resolver.

| Published component | Current owner | Required persistence class | Status |
| --- | --- | --- | --- |
| File inventory rows | `FileInventory.files` | eager archived metadata | eager and round-trips; encoded inside rkyv-framed metadata chunks as bounded serialized snapshot data |
| File versions and active-file bitmap | `FileVersions`, `FileCatalog.active` | eager metadata | eager and round-trips |
| Record rows and segment file membership | `SegmentRowsOwner` | eager archived metadata | eager and round-trips; native row reconstruction remains |
| File and RecordId lookups | `FileLookup`, `RecordLookup` | eager archived hash maps | eager and round-trips; native lookup reconstruction remains |
| Catalog metadata/path/ownership shards | `FileCatalog` | eager archived shards | eager and round-trips; shard-level disk reuse is not implemented |
| Base and current segment identity/order | collection generations, segment groups | eager metadata | eager and round-trips |
| Declared/inferred/custom posting dictionaries | `PostingDictionary` | lazy archived dictionary frame | per-field dictionary holds key ordinals and independently addressed leaf descriptors |
| Posting reverse directory | `OrderedPostingArchive` | lazy within dictionary frame | native/archived dictionaries retain their existing reverse directory; stored dictionaries use the ordered key map |
| Posting bitmaps | canonical `Arc<Vec<RoaringBitmap>>` | lazy Roaring leaves | per-value, length and structural leaves carry offset, length, cardinality, decoded estimate and SHA-256; selected equality terms load only their leaf |
| Presence/null/empty-selection masks | `PostingIndex` | eager liveness/state plus lazy query-state leaves | presence/null/empty selection are eager in snapshot metadata; structural/length/value payload is field-lazy |
| FTS normal/skip dictionaries | `BigramIndex` | lazy archived dictionary frame | per-binding dictionary frame implemented; normal and skip maps are separately keyed |
| FTS bitmaps | `BigramIndex` leaf pack | lazy Roaring leaves | fully lazy per-gram leaves implemented with offset, length, cardinality, decoded estimate and SHA-256; bounded coalescing and decoded cache implemented |
| Sorting keys/order arrays | `SortCollection` | lazy sort frames | collection/segment-level rkyv payload implemented; per-column independent frames remain pending |
| Base relations and contributions | `RelationIndex` / `RelationContribution` | lazy relation payload | base CSR adjacency is a checked lazy frame; segment-local contributions remain eager metadata and endpoint-level splits are pending |
| Inferred-order and verified caches | synchronized runtime caches | ephemeral/not persisted | classified |
| Worker pools/query plans | runtime only | ephemeral/not persisted | classified |
| Source readers | runtime registration | identity persisted; object rebound at open | pending |

## Current lazy units

- FTS dictionary: one lazy unit per field/binding/segment representation.
- FTS normal/skip leaf: one lazy unit per gram, addressed by the dictionary frame.
- Posting: one dictionary unit per field and one independently decoded Roaring leaf per value/length/structural key.
- Sort: one coarse lazy unit per collection/segment sort collection.
- Relation: one lazy unit per base relation declaration.

The benchmark confirms selected FTS bindings and leaves stay cold until execution. A
selected query loads the relevant segment dictionaries and only the requested gram or
posting leaves; unrelated fields remain cold. Sort components still require the
per-column refactor.
