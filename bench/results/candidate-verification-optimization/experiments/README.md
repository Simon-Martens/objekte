# Independent parser and matcher experiments

These files were produced in the JSON and matcher subtask worktrees on the
same machine and fixed fixtures as the focused baseline. They are exploratory
variant results; the final accepted combination is recorded in the parent
directory's `integration-*.jsonl` files.

- `parser-*.jsonl`: combined query extraction before delimiter optimization
  (`3a28f38f7e`).
- `memchr-*.jsonl`: repeated `memchr2` for quote/backslash, rejected because
  long escaped strings slowed substantially.
- `memchr-quote-*.jsonl`: quote-only delimiter search, rejected because the
  escape-heavy path regressed.
- `memchr-hybrid-*.jsonl`: exploratory quote-only first-search fallback.
- `memchr2-hybrid-*.jsonl`: accepted first quote/backslash `memchr2`, followed
  by a byte loop after the first escape (`034ade87de`).
- `matcher-microbench.log`: release microbenchmarks for owned Finder matching,
  wildcard pieces, case-fold scratch, and Finder preparation. The test used to
  produce the log was temporary and was removed before the matcher commit.

The independent variant medians and caveats are summarized in
[`docs/ai/generated/candidate_verification_optimization_results.md`](../../../../docs/ai/generated/candidate_verification_optimization_results.md).
