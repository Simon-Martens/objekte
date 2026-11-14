# Direct snapshot access format contract

This contract records the wire layout before the corresponding decoders are changed. All integers are unsigned little-endian integers. Offsets are section relative unless explicitly described otherwise. Every length and multiplication is checked before slicing. No native pointers, `usize`, Rust collection headers, or randomized hash state are stored.

## Segment envelope revision 3: gram filters

The segment header keeps the existing `OBJHSEG1` magic, 24-byte header, and 56-byte section directory entries. Its format version becomes 3. The existing ten section kinds retain their numbers and a required `GramFilters` section is added as kind 11. It may be empty. Its whole-section SHA-256 is verified on first access by `SegmentReader.section`, so opening a filter currently touches all filter bytes. The segment and generation checksums bind the directory and section contents; they do not prove semantic coverage.

The small bincode metadata sections retain their magic values and change their shared metadata version to 2. `TextBindings` adds two spans to each serialized `TextBinding`, `normal_filter` and `skip_filter`, after the normal/skip dictionary spans. Each span is two `u64` values, section-relative start and byte length, in that order. A zero length means no filter; its start must equal the position reached by preceding filter spans. All nonempty spans are contiguous in binding order, normal then skip, without overlap or trailing bytes. The binding's vector ordinal, alphabet, and dictionary spans bind each filter record.

Each nonempty filter span is exactly a 48-byte header plus `word_count * 8` bytes:

| Offset | Width | Meaning |
| ---: | ---: | --- |
| 0 | 8 | ASCII magic `OBJHGRM1` |
| 8 | 4 | filter format version, 1 |
| 12 | 4 | hash algorithm version, 1 |
| 16 | 4 | text-binding ordinal, zero based |
| 20 | 4 | family: 0 normal, 1 skip |
| 24 | 8 | dictionary start in `BigramDictionaries` |
| 32 | 8 | dictionary length |
| 40 | 4 | dictionary key count |
| 44 | 4 | word count |
| 48 | `word_count * 8` | little-endian `u64` bit words |

Only dictionaries with at least 32 keys have a filter. For `n` keys, the bit count is `next_power_of_two(clamp(max(12*n, 64), max=2^26))`, the word count is bit count / 64, and the bit mask is bit count minus one. `n` must fit `u32`. The writer takes the key count from the builder map or sealed dictionary header; it inserts every writer-visited key into three bit positions and emits the record after the dictionary is finished. The three hashes use the existing splitmix-style mix and odd rotated step in `GramPresence`; hash algorithm version 1 freezes those constants and the order `[0, 1, 2]`. Bit `b` resides in word `b / 64`, bit position `b % 64`.

The reader checks magic, versions, lengths, counts, dictionary span, binding ordinal, family, and section bounds before using a filter. The alphabet and segment identity are bound by the containing checked `TextBinding` and segment envelope. A filter with a missing or invalid record never proves absence. On an untrusted open, the existing dictionary validation walk checks that every decoded key passes the filter, without constructing a second filter or visiting the dictionary again. A matching current validator stamp can reuse this coverage proof; changing the filter semantics bumps the validator version. Constructed readers inherit the writer's proof.

## Later metadata revisions

The later [record-slot contract](direct_snapshot_segment_format.md) and [generation-catalog contract](direct_snapshot_catalog_format.md) specify the completed direct-access layouts. Their validators check ordering, membership, path semantics, and cross-table references in addition to checksums. This section records the earlier gram-filter revision; the containing segment format is now version 5.
