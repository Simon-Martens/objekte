# Short-string automatic indexing contract

Starting development revision: `6f83bf06aa21477cfb2b19b0e66a4df97cd6e745`.

- `InferenceMode::Disabled` remains the CLI and library default. `obj --auto-index` selects `AutoIndex` for its one collection; `--schema FILE` independently supplies explicit declarations and manual indexes. The command opens the repository once.
- An eligible logical string has at most 64 decoded UTF-8 bytes, inclusive. UUID spellings are ordinary strings. The existing low-cardinality rule remains independent for strings beyond this threshold when its configured limit permits them.
- Inferred string keys use the existing byte-lexicographic `String`/`ScalarKey::String` ordering. Only identity, compatible equality and range operations may use these postings. Case folding, collations and custom transforms keep their existing paths.
- A short-string dictionary is per collection and canonical path. A record is fully covered for a string predicate only when its complete selection at that path consists of represented short strings. Long, mixed, null, missing, empty and unavailable selections remain in the unresolved domain and undergo ordinary evaluation. Inferred hits are candidates, never exact predicate proofs.
- Explicit indexed fields reuse their declared posting; explicit indexing prohibitions exclude automatic postings. An absent `index` annotation does not prohibit inference. The JSON Schema adapter accepts local files and preserves the CLI root, reader and relative-path identity.
- The policy retains bounded path/depth/contribution, per-batch and repository budgets. Policy version 3 uses a 4 MiB batch estimate, 64 MiB coordinator estimate, 128 MiB repository estimate, and 24 MiB per short-string path. The larger batch allowance keeps ordinary multi-field 512-file batches eligible for automatic indexes. These are estimates of retained index data, not process RSS limits. The short-string limit and coverage representation are fingerprinted with the inference policy. Publication and snapshot validation check coverage bounds.
- `obj --tui` reuses the existing terminal components with a single already opened repository. Its table has permanent `valid`, `id`, `match` columns plus columns derived from the bound filter and sort. It suppresses relation navigation and hyperlinks while leaving source values visible in detail.

This contract records intended behavior before implementation; measurements and final capabilities belong in the acceptance report.
