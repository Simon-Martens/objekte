# Component coverage inventory

This inventory is the phase-1 classification ledger. It is updated as each
component gains its stored descriptor and resolver.

| Published component | Current owner | Required persistence class | Status |
| --- | --- | --- | --- |
| File inventory rows | `FileInventory.files` | eager archived metadata | pending |
| File versions and active-file bitmap | `FileVersions`, `FileCatalog.active` | eager metadata | pending |
| Record rows and segment file membership | `SegmentRowsOwner` | eager archived metadata | pending |
| File and RecordId lookups | `FileLookup`, `RecordLookup` | eager archived hash maps | pending |
| Catalog metadata/path/ownership shards | `FileCatalog` | eager archived shards | pending |
| Base and current segment identity/order | collection generations, segment groups | eager metadata | pending |
| Declared/inferred/custom posting dictionaries | `PostingDictionary` | lazy archived dictionary frame | pending |
| Posting reverse directory | `OrderedPostingArchive` | lazy within dictionary frame | pending |
| Posting bitmaps | canonical `Arc<Vec<RoaringBitmap>>` | lazy Roaring leaves | pending |
| Presence/null/structural/length state bitmaps | `PostingIndex` | lazy Roaring leaves/state descriptors | pending |
| FTS normal/skip dictionaries | `BigramIndex` | lazy archived dictionary frames | pending |
| FTS bitmaps | `BigramIndex` | lazy Roaring leaves | pending |
| Sorting keys/order arrays | `SortCollection` | lazy sort frames | pending |
| Relations and contributions | `RelationIndex` / `RelationContribution` | lazy relation payload | pending |
| Inferred-order and verified caches | synchronized runtime caches | ephemeral/not persisted | classified |
| Worker pools/query plans | runtime only | ephemeral/not persisted | classified |
| Source readers | runtime registration | identity persisted; object rebound at open | pending |
