# Native, mixed, and archived ownership

## Published native

- Dictionary maps use `BTreeMap<StoredKey, u32>` with a canonical
  `Vec<RoaringBitmap>` payload array.
- Published native maps and payloads are shared through `Arc`; no second
  B-tree is constructed at archive time.
- Raw/fallback/custom text and sorting payloads use `Arc` owners in the base
  repository and segment owners. Segment rows share one immutable owner.
- Native startup creates no checked archive buffers. The focused archive tests
  assert `temporary_ordinal_trees == 0` and `checked_archive_validations > 0`
  only after an explicit archive call.

## Explicit archive

- Posting B-tree maps and ascending reverse-directory entries are serialized
  from borrowed native iterators.
- File and record lookup hash maps are serialized from their native iterators.
- Catalog exact/ordered maps, metadata rows, and ownership maps are serialized
  from borrowed native shard owners.
- The canonical bitmap array, text payloads, sorting payloads, liveness owners,
  and relation payloads remain shared native owners. They are reported as
  residual native payload bytes, not mislabeled as archived dictionaries.
- Old native rows/maps remain reachable only through pinned pre-archive views.
  The focused lifecycle test drops a pinned native view and verifies its
  segment owner is released.

## Residual native categories

From the ASCII final archive sample:

| Category | Bytes |
| --- | ---: |
| bigram/raw/fallback/custom text | 86,790,604 |
| sorting keys and columns | 23,660,638 |
| relations | 0 |
| active/control and compatibility metadata | 13,066,768 |

The archive itself is 149,111,376 serialized bytes with 233,891,392 allocated
bytes, leaving 84,780,016 bytes of capacity slack. These are current-owner
diagnostics, not claims about immediate RSS reclamation.
