# RecordSlots direct-reader contract

This document specifies `RecordSlots` revision 2 before its decoder is changed. It extends [the segment format contract](direct_snapshot_access_format.md). The containing segment format version must advance to 4, and the validator stamp version to 3, because revision 2 cannot be interpreted by the previous record decoder. All integers are little-endian unsigned scalars. The format has no alignment padding except the three fixed zero bytes in each record row. All offsets are section relative or relative to their named arena. No Rust `usize`, collection header, pointer, or randomized hash state is stored.

## Section structure

The section is at most 512 MiB. Its first 16 bytes are ASCII `OBJHREC2`, section version `u32 = 2`, and model version `u32`. The next fields are:

| Order | Type | Meaning |
| --- | --- | --- |
| 1 | `u32` | collection ID |
| 2 | `u32` + bytes | nonempty UTF-8 source ID, at most 64 KiB |
| 3 | `u32` | source version |
| 4 | `u32` | file row count, at most 1,048,576 |
| 5 | `u32` | record row count, at most 1,048,576 |
| 6 | `u32` | ID arena byte count |
| 7 | `u32` | membership slot count; equals record row count |
| 8 | `u32` | portable Roaring logical-domain byte count |
| 9 | `u32` | portable Roaring fallback-domain byte count |

Immediately thereafter, with no gaps, come the file rows (`16 * file_count` bytes), record rows (`36 * record_count` bytes), membership slots (`4 * record_count` bytes), ID arena (`id_bytes`), logical-domain bytes, and fallback-domain bytes. The section ends exactly there. Every count multiplication and addition is checked against the section extent before any slice is formed.

File rows are sorted by strictly increasing numeric `FileId` and have this layout:

| Offset | Width | Meaning |
| ---: | ---: | --- |
| 0 | 4 | `FileId` |
| 4 | 4 | catalog metadata reference; current ID or tagged historical reference |
| 8 | 4 | first membership slot index |
| 12 | 4 | membership slot count |

Membership descriptors partition the membership array contiguously in file-row order, including zero-record files. Each file's slot list is strictly increasing and may be noncontiguous in the global record table. Every slot from zero through `record_count - 1` occurs exactly once. Its referenced record row names that file row. A checked streaming pass and a temporary `record_count`-bit validation bitmap establish completeness; that bitmap is discarded before publication.

Record row number is its `RecordSlot`; no slot value is duplicated in a row. Rows have this layout:

| Offset | Width | Meaning |
| ---: | ---: | --- |
| 0 | 4 | file-row index, less than file count |
| 4 | 8 | source byte-range start |
| 12 | 8 | source byte-range end |
| 20 | 1 | class: 0 logical, 1 raw fallback |
| 21 | 3 | zero reserved bytes |
| 24 | 4 | diagnostic reference |
| 28 | 4 | ID arena byte offset |
| 32 | 4 | ID byte length, 1 through 65,536 |

ID byte ranges partition the arena exactly in record-slot order, with no overlap or unreferenced bytes. Each ID is valid UTF-8. Exact comparison is the existing `RecordId` string ordering (UTF-8 lexical order); no locale or case fold is applied to this segment-local lookup. The already persisted `RelationAndIdentity` slot permutation is checked for completeness and strict ID ordering and is used for binary search. There is no duplicate record-ID dictionary. A record row can be located in constant time and its ID slice does not require decoding preceding rows.

The diagnostic reference retains the existing `DiagnosticRef` convention: `u32::MAX` means no stored diagnostic; otherwise it indexes the referenced file's diagnostics. File IDs and catalog references have no missing-value sentinel in a present file row. The file table itself may be empty only when the record table and membership table are empty.

The logical and fallback domains use the portable Roaring serialization already used for postings. Their byte counts are explicit, their decoded cardinalities sum to `record_count`, they are disjoint, and each record row's class agrees with exactly one domain. The two bitmaps may be decoded into bounded Roaring values for existing set operations. These immutable domains never incorporate mutable active masks.

## Reader, writer, and validation boundaries

The writer consumes already admitted file rows, slot memberships, record rows, and logical domain. It sorts file descriptors by numeric file ID once, remaps each record's file-row index, and writes IDs into a contiguous arena while writing record rows. It writes the portable domains without deriving them on reopen. Memory-only readiness keeps its native constructed tables; this encoding is deferred until durable persistence.

The reader retains an `Arc<SegmentReader>` for mapped or resident section bytes and an `Arc<RestoredGenerationCatalog>` for checked metadata references. It installs counts and offsets, not owned record/file vectors or ID/file hash maps. One record-by-slot or file-by-ID lookup reads a bounded row and, where needed, decodes one owned `RecordEntry` or `FileEntry` at an ownership boundary. Ordinarily it borrows the UTF-8 ID slice. It never materializes all rows on the first query or mutation. A constructed segment continues to use its existing native vectors and maps.

`SegmentReader.section(RecordSlots)` currently verifies SHA-256 across the full section before exposing it. External opens then traverse file and record rows once for metadata references, source ranges, diagnostics, class, ID UTF-8, fallback identity, membership agreement, and domain agreement. The persisted identity permutation is checked in its own existing relation section pass; that check must compare borrowed ID slices rather than construct an ID map. A validator stamp can omit checks already proven for the exact committed generation only when its version is current. Every malformed byte range, reference, identity, or domain is a typed corruption error, never a no-match result. Temporary validation scratch is not installed as a lookup.

The segment reader's catalog reference is immutable and retained by each pinned generation. A replacement catalog or file move cannot retire bytes while a pinned query uses them. The catalog's current/historical reference tag semantics remain authoritative; a segment file descriptor records the exact version it indexed.
