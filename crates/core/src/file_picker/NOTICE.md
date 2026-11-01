# File picker provenance

This module applies FFF's file-picker design to Objékte's flat JSON
collections: stable numeric slots, a separate sorted file view, an ID byte
arena, an immutable bigram index, and a changed-file overlay. The code here is
an adaptation for this repository, not a copy of FFF's full `FilePicker`.

The bigram builder and filter are now copied into [bigram](../bigram/) from
FFF at commit `d1c7dd47130ca670ab71e7466a1a3b4a57d5348d`; see its
[license](../bigram/LICENSE) and [notice](../bigram/NOTICE.md). `regex_plan.rs`
copies and adapts the regex-to-bigram planner from FFF's `bigram_query.rs` at
that commit. Its private column lookup uses the local `BigramFilter::query`.

FFF's general walker handles nested paths, ignore rules, symlinks, and Git
metadata. Objékte collections are flat directories containing JSON records,
so this module uses the existing `Source::inventory` scanner. It parallelizes
metadata reads, stores IDs once in a byte arena, and builds both of FFF's
bigram posting indexes from decoded JSON values.

FFF's grep path reads or maps each candidate once, performs a fast whole-file
literal check before line-oriented matching, and reuses a buffer per worker.
The explicit `bigram` query backend uses the picker for candidate selection,
then reuses one source buffer per worker for the source regex scan and Sonic
verification.
