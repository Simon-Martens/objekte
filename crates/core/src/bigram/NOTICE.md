# FFF-derived code

Two files are adapted from FFF at commit
`d1c7dd47130ca670ab71e7466a1a3b4a57d5348d` and remain under the MIT license in
[LICENSE](LICENSE):

- [`column_slab.rs`](column_slab.rs), from FFF's
  `crates/fff-core/src/index/column_slab.rs`.
- [`../file_picker/regex_plan.rs`](../file_picker/regex_plan.rs), the
  regex-to-bigram planner from FFF's `bigram_query.rs`. Its private column
  lookup is replaced with the local `BigramFilter::query`.

The rest of the bigram module and the file picker are Objékte's own code,
under the Unlicense.
