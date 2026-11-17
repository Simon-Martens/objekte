# Metadata and payload ownership

Task: `rkyv-construction-performance`

This inventory describes the query representation reached by the matched
32-worker Unicode construction sample.

| Structure | Owner and construction site | Readers | Shared or copied |
| --- | --- | --- | --- |
| Posting B-tree and reverse directory | `PostingDictionary`; `seal_completed_storage` | Executor, value/range enumeration, diagnostics | One validated archive per nonempty dictionary. Empty dictionaries share one process-wide archive for each key type. |
| Posting bitmaps | `PostingDictionary.bitmaps` sidecar | Executor and planner | Original moved `RoaringBitmap` values; not serialized into the dictionary archive. |
| Segment file and record rows | `SegmentRows` archive split into `SegmentFiles` and `RecordRows` | Query scanning, file/record metadata, ownership routes | Both views share one `Arc<ValidatedArchive>`. Compatibility vectors remain lazy. |
| File and record lookups | `FileLookup` and `RecordLookup` | Direct file/ID lookup | Each owns one validated archive. Existing local maps are consumed when frozen. |
| Catalog metadata shards | `FileCatalog.metadata` | File status, revision, diagnostics, path reconstruction | 1,024-ID archived shards; unchanged `Arc` owners survive rebase. |
| Catalog path shards | `FileCatalog.paths` | Exact and ordered path lookup | 1,024 hash-selected archived shards; unchanged owners survive rebase. |
| Ownership shards | `FileCatalog.ownership` | File retirement and relocation | Versioned routes keyed in 1,024-ID shards; preserved across final catalog rebase. |
| Base inventory | `FileCatalog.inventory`, repository inventory | Public inventory compatibility and discovery metadata | Existing `Arc<FileInventory>` is shared; native lookup maps are released after archived-path adoption. |
| Raw and fallback text | `Arc<BigramIndex>` in the segment | FTS | Inference-only refreshes share the owners. |
| Sorting | `Arc<SortCollection>` in the segment | Stored ordering and metadata sorts | Inference-only refreshes share the owner; status edits copy only when orders must change. |
| Relations | Segment contribution vectors plus archived relation posting dictionaries | Relation traversal | Native relation source references remain required for merged views. |
| Collection identity compatibility | `CollectionState.generation`, `id_lookup`, and `identity_order` | Public record metadata and direct identity lookup | One canonical native compatibility copy per collection. |
| Construction duplicate registry | `PublishedIndex.construction_ids` | Initial duplicate-ID admission only | Temporary `HashSet<Arc<str>>`; released before final publication. |

## Full-corpus accounting

From the 32-worker Unicode follow-up sample:

| Category | Bytes |
| --- | ---: |
| Unique serialized archives | 148,561,552 |
| Unique aligned archive capacity | 233,658,336 |
| Capacity slack | 85,096,784 |
| Posting bitmap payload estimate | 17,713,236 |
| Bigram payload estimate | 91,134,554 |
| Sorting payload estimate | 23,660,638 |
| Native inventory estimate | 40,275,669 |
| Initialized file-row caches | 0 |
| Initialized record-row caches | 0 |

The predecessor reports 4,757 unique archive owners. Empty-archive sharing and
unchanged path/catalog reuse reduce that to 2,594 owners, while the complete
logical representation and all metadata categories remain present.
