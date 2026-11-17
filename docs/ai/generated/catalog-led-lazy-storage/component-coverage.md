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
| Declared/inferred/custom posting dictionaries | `PostingDictionary` | lazy archived dictionary frame | field-level rkyv payload implemented; one frame currently includes all field leaves |
| Posting reverse directory | `OrderedPostingArchive` | lazy within dictionary frame | constructed after payload load; not directly persisted |
| Posting bitmaps | canonical `Arc<Vec<RoaringBitmap>>` | lazy Roaring leaves | serialized inside a field component; independent per-leaf catalog/loading remains pending |
| Presence/null/structural/length state bitmaps | `PostingIndex` | lazy Roaring leaves/state descriptors | presence/null masks are eager; value/length/structural payload is lazy in one field frame |
| FTS normal/skip dictionaries | `BigramIndex` | lazy archived dictionary frames | binding-level rkyv payload implemented; normal/skip split is inside one field frame |
| FTS bitmaps | `BigramIndex` | lazy Roaring leaves | serialized with the binding payload; independent per-gram leaves remain pending |
| Sorting keys/order arrays | `SortCollection` | lazy sort frames | collection/segment-level rkyv payload implemented |
| Relations and contributions | `RelationIndex` / `RelationContribution` | lazy relation payload | serialized eagerly in metadata; lazy relation payload remains pending |
| Inferred-order and verified caches | synchronized runtime caches | ephemeral/not persisted | classified |
| Worker pools/query plans | runtime only | ephemeral/not persisted | classified |
| Source readers | runtime registration | identity persisted; object rebound at open | pending |
