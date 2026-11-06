# Core foundation correction baseline

Starting code: `964fc6385c9d377e170317b5e39d31c606537623`.
The current corrected query and construction recordings under
`bench/results/musenalm/final-084623a19b/` and `build-084623a19b/`
measure its code parent, `084623a19bf26fc56b470b6548c38382dff4d880`.
They are the immediate performance baseline; the older `5887c7be02`
recordings are historical.

## Locally inspected reproduction paths

- `executor.rs` selects the raw bigram index for a `default` field substring
  or selected FTS field without checking the raw binding or reader text
  coverage. `repository.rs` may transform those raw segments. Focused
  independent-ID fixtures will reproduce the false negatives.
- `reference.rs` builds `temporal_validation` using only temporal declarations.
  A required temporal child below an optional object therefore makes an absent
  parent appear required during query conversion.
- `posting.rs` adds every reached array length, whereas `reference.rs::eval_term`
  checks only the first. The existing posting length union proves an
  existential condition, while the default multi-valued comparator is
  universal.
- `BindingOperation` prepares literals and maps values/text, but exact
  verification uses built-in comparisons and allocated `map_text` results.
- Build and query paths replace reader-owned text with `text_segments` when
  temporal fields exist.
- The stored-order path calls `scan_file` or `direct_payload` per record;
  both create a new range reader. Single-file deep pages consequently reopen
  the source for each evaluated record.

These are code-path reproductions, not yet claims of corrected behavior.
Focused failing tests and independent expected results accompany the relevant
fix commits. The current implementation rejects malformed selected records
at construction and exposes no posting-value enumeration API.

## Completion checks

- [ ] Compatible bigram access paths and conservative coverage.
- [ ] Optional temporal ancestors and projected validation.
- [ ] Nested array length quantifiers and safe posting proofs.
- [ ] Prepared custom exact scanners and reader-owned text.
- [ ] Shared preparation/materialization and bounded ordered source access.
- [ ] Reader range validation, status, diagnostics and raw fallback search.
- [ ] Posting values, exact selection counts and bounded continuation.
- [ ] Cross-feature, differential, boundary and resource tests.
- [ ] Full formatting, lint, tests, final 150-query, construction, restore and
      focused performance evidence.

The source-read race exclusion in `fix2.md` remains in force. The worktree's
pre-existing documentation move is deliberately outside this branch's commits.
