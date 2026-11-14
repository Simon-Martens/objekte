# Direct generation catalog format, version 1

This contract describes the current generation catalog saved by `PROGRAM_VERSION = 1`. It extends the segment format contract in `direct_snapshot_access_format.md`. Integers are unsigned little endian except signed nanosecond fields, which are signed 128-bit little endian. Every offset is checked with `checked_add`/`checked_mul` and every slice is bounded before use. The maximum catalog file is 512 MiB, so the 31-bit arena offsets below suffice. There is no native Rust layout, pointer, `usize`, or hash state on disk.

## Envelope and body header

The 28-byte envelope is `OBJHCAT1` (8 bytes), `PROGRAM_VERSION` (`u32 = 1`), body length (`u64`), expected segment count (`u32`), and reserved zero (`u32`). There is no mandatory body or whole-file SHA-256; the catalog is checked by a positive ownership check, envelope lengths and complete internal directory validation. Whole-file verification is available only through an explicit manifest `[verify]` entry.

The body starts with a 104-byte header:

| Offset | Width | Field |
| ---: | ---: | --- |
| 0 | 32 | configuration identity |
| 32 | 8 | publication revision, nonzero |
| 40 | 8 | next segment ordinal, nonzero |
| 48 | 4 | collection count, nonzero |
| 52 | 4 | allocated file slots, including retired holes |
| 56 | 4 | historical metadata entries |
| 60 | 4 | active file count |
| 64 | 4 | slot table start |
| 68 | 4 | historical offset table start |
| 72 | 4 | native path order table start |
| 76 | 4 | query path order table start |
| 80 | 4 | metadata arena start |
| 84 | 4 | separate query-key arena start |
| 88 | 4 | active bitmap start |
| 92 | 4 | inference summaries start |
| 96 | 4 | body end |
| 100 | 4 | reserved zero |

These offsets must be monotone and adjacent to their preceding fixed table. The slot table starts at 104; historical starts after `16 * slot_count`; native after `8 * historical_count`; query after `4 * active_count`; metadata after `12 * active_count`. The variable metadata arena ends at the query-key arena; the query arena ends at the active bitmap; the bitmap ends at inference; inference ends at the declared body end, which equals the body length. No region may overlap or contain unreferenced bytes. All counts are bounded by 16,000,000 and the existing section-size limit.

## Dense slots and metadata references

Each 16-byte current slot row is: file version `u64`, metadata offset `u32`, metadata length `u32`. Versions are in `1..=publication_revision`. A retired slot has offset `u32::MAX` and zero length; all other rows have an offset relative to the metadata arena and nonzero length. The current metadata record is the existing canonical `FileEntry` encoding followed by two optional 32-byte hashes (fingerprint then configuration, each preceded by a one-byte marker). Its encoded FileId must equal the slot number. Current records occupy the first part of the metadata arena in file-ID order, with no gaps.

Each 8-byte historical row is a metadata offset `u32` and nonzero length `u32`, also relative to the metadata arena. Historical records follow current records in reference order without gaps and use the existing canonical `FileEntry` encoding without hashes. The metadata reference's high bit still selects the historical table; the low 31 bits select its row. An untagged reference selects a current slot and cannot resolve a retired slot. Historical file IDs must be in the allocated slot range. Duplicate historical entries for the same file ID are invalid. Historical references remain stable across generations unless the existing safe rebase rewrites referencing segments.

The canonical `FileEntry` encoding begins with `file_id:u32`, `native_path_length:u32`, then native path bytes (Unix `OsStrExt::as_bytes`, Windows UTF-16 code units as little-endian bytes). It continues with `modified_ns:i128`, `size:u64`, optional `created_ns`, membership count and `u32` collection IDs, status `u8`, omitted diagnostic count `u32`, diagnostic count `u32`, and length-framed JSON diagnostic leaves. Existing limits remain: native path 64 KiB, source ID 256 bytes, diagnostic leaf 16 KiB, and the bounded diagnostic count. All parser and status/membership invariants remain mandatory. Bounded leaves may be decoded as needed; opening must not retain an owned `FileEntry` for every row.

The file version is directly readable from the slot row. Runtime mutations keep only changed version tokens in a bounded overlay. Reading a version from a valid unchanged slot does not decode its metadata.

## Two path comparators

The native table is `active_count` file IDs (`u32`) sorted strictly by the existing native `Path`/`OsStr` ordering. Native exact lookup compares against the borrowed path bytes in the referenced current metadata record on Unix; Windows decodes a bounded path for comparison. This table preserves non-UTF-8 paths and does not use the escaped query spelling as identity.

The query table is `active_count` 12-byte rows: file ID `u32`, key offset `u32`, key byte length `u32`. Rows are strictly sorted by `(SharedPath::query_key(), file_id)`; the file ID is a deterministic tie breaker when distinct native paths share a query spelling. A query equality/range must retain every such file ID. The high bit of the offset selects the separate query-key arena; the remaining 31 bits are relative to that arena. A clear high bit selects bytes within the metadata arena, ordinarily the native path bytes of a plain UTF-8 path. Key lengths are bounded by the existing path/query limits, and each row's key must exactly equal the query spelling derived from its file's native path. Duplicate native paths are invalid, while duplicate query keys are allowed. The writer sorts once for each genuinely different comparator, reusing those orders for uniqueness validation and encoding. The decoder walks each sorted table once to check order, file-ID coverage, and key/path agreement; it never sorts at open.

The key arena contains only separate escaped query keys, concatenated in query-table order with no padding. Plain paths share their already encoded native bytes. `str` byte comparison is valid for the query table because keys are UTF-8 and the established query comparator is Rust `str` lexicographic order. It is not used for native path identity.

## Active state and inference

The active region is a portable Roaring bitmap of current live file IDs, serialized by the existing Roaring writer. Its cardinality equals `active_count`, all members are below `slot_count`, and membership agrees exactly with non-retired slot rows and both path tables. It is decoded once into the existing bounded runtime bitmap; no bitmap is rebuilt by scanning slots. This immutable domain is distinct from each segment's mutable active-record mask.

The inference region keeps one existing length-framed fixed-int bincode summary per configured collection in collection order. Those small configuration summaries may be eagerly decoded and validated. Structural checks run before publication; later file/path lookups use the immutable owner plus small mutation overlays. A checksum alone would not be a proof of path order, membership, or metadata-reference consistency.
