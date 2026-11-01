# FFF bigram code

`filter.rs` and `column_slab.rs` are adapted from FFF's
`crates/fff-core/src/index/bigram_filter.rs` and
`crates/fff-core/src/index/column_slab.rs` at commit
`d1c7dd47130ca670ab71e7466a1a3b4a57d5348d`.

The builder, compressed filter, sparse encoding, normalization, changed-file
overlay, memory slab, and their unit tests are retained. FFF's `FileItem` file
walker, binary sniffing, and path-arena integration are replaced by
`build_bigram_index_from_content`, which accepts content in caller-owned slot
order. Objékte's file picker also feeds the builder directly.

FFF is licensed under the MIT license in [LICENSE](LICENSE).
