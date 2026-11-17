# Component coverage inventory

Final implementation revision: `ee52c8239c`

This table classifies every published component at the final storage revision.
`Archived eager` means the component is restored and checked before search-ready.
`Lazy` means a selected query opens it on first use.

| Published component | Stored representation | Status |
| --- | --- | --- |
| File rows, lossless paths, versions, status, diagnostics | Separate checked rkyv `EagerFileCatalog` with row, FileId and path lookup tables | Archived eager and validated before publication |
| Active file masks and file versions | Eager snapshot metadata plus validated catalog routes | Archived eager |
| Record rows, segment ownership and file membership | Separate checked rkyv `EagerRecordCatalog` with segment groups, rows and lookup rows | Archived eager and cross-checked against file/index/leaf catalogs |
| Collection/segment/identity order | Eager snapshot metadata | Archived eager |
| Existing metadata/path/ownership catalog shards | Existing checked rkyv lifecycle catalog shards | Restored eagerly; shard-level reuse is retained |
| Index capabilities, coverage and lazy component locations | Separate checked rkyv `EagerIndexCatalog` | Archived eager, including declared/inferred/custom posting, FTS, sort and relation descriptors |
| Leaf directories | Separate checked rkyv `EagerLeafCatalog`; dense row arrays address posting and FTS leaves by group/pack | Archived eager and bounds/overlap checked |
| Declared/inferred/custom posting dictionaries | Per-field lazy checked rkyv dictionary frame | Lazy per field/binding; reverse/ordered key directory retained |
| Posting bitmaps | Independent standard Roaring leaves with offset, length, cardinality, decoded estimate and SHA-256 | Lazy per selected value, length or structural key; weak decoded ownership follows the bounded storage cache |
| Presence/null/empty selection | Eager snapshot state masks | Eager for correctness. Independent state-leaf loading is not implemented; this is a documented deviation from the query-specific-leaf layout |
| FTS normal/skip dictionaries | Per-binding lazy checked rkyv dictionary frame | Normal and skip maps remain distinct, including alphabet, binding, raw/fallback and normalization identities |
| FTS bitmaps | Independent Roaring leaves with bounded coalescing and early intersection stop | Lazy per gram; missing complete gram returns empty without sibling reads |
| Sort keys and order arrays | Independent checked frames per configured column and composite order | Lazy on selected sort; ascending, descending, ties, nulls/missing and composites preserved |
| Base relation adjacency | Checked lazy relation payload | Lazy on first selected relation use |
| Segment-local relations/backlinks | Relation metadata in snapshot plus base lazy payload | Correct round trips; not split into independent endpoint contributions, so this remains coarser than the target layout |
| Registered reader/extractor identity and version | Manifest/configuration identity | Validated before open; incompatible registration fails explicitly |
| Verified-result, relation-target and inferred-order caches | Process memory only | Ephemeral and never persisted |
| Worker pools, query plans and source objects | Process memory only | Ephemeral; source readers are rebound from persisted identity |
| Snapshot runtime details | Checked rkyv-framed metadata byte objects containing a JSON snapshot | Correct and bounded at object framing, but eager JSON decode remains a measured schema/performance gap |

## Cache and ownership

- `KeepLoaded` is the default; `Budget { resident_bytes }` is optional.
- Query, context and streaming entry points pin the storage cache for the duration
  of execution. `trim_storage_cache` and post-unpin budget enforcement do not evict
  while a query pin is active.
- Stored posting bitmaps are borrowed from cache-backed `Arc` leases. Eviction can
  release every stored leaf; later selected access performs a new checked read and
  decode. Native and archived dictionaries still return borrowed leaves without a
  per-leaf `Arc` allocation.
- The pack-handle cache is bounded at 64 entries. Active positioned reads retain their
  own file handle after handle-cache eviction.
- Eager catalogs, unsaved native payloads and active query pins are outside the payload
  budget. The reported cache budget is therefore not a total-process RSS cap.

## Known limitations

Presence/null state is eager, segment-local relation metadata is not fully split, and
the runtime snapshot is still decoded from JSON carried in checked rkyv frames. These
are correctness-preserving implementation limitations with measured eager-open and
memory cost, not claims that those components already have their final lazy schema.
