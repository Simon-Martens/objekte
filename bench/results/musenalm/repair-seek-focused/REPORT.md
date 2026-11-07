# Positioned reader focused comparison

Unmodified A: `92cdb92b19200cb0005af23b66aeb3cd1b514c4b` in a separate
sparse worktree. Changed B: `fix/core-foundation-performance`, starting at
`6e1e96d9ea`, with the positioned reader patch in the working tree. The B
binary was compiled before the final formatting pass; formatting made no
semantic edits. The runner's `code_revision` field names the clean Git parent
and therefore does not identify the uncommitted B binary by itself. The code
commit accompanying this report identifies the patch.

Both release binaries ran against the same physical Musenalm corpus with Rust
1.99.1, 32 workers, ASCII, verified cache disabled, and zero source/decoded
cache. Process order was A-B-B-A, with 11 timed repetitions per case in each
process block, after each runner's normal build and warmup. Each block retains
its exact command, run manifest, raw samples, per-query summaries, plans and
console output. The OS file cache was warm. Host affinity was `0-31` and the
governor was `powersave`; no power setting was changed.

| Query | A block medians ms | B block medians ms | Pooled A median ms | Pooled B median ms | Difference ms |
|---|---:|---:|---:|---:|---:|
| Q006 | 232.486, 231.982 | 227.286, 226.403 | 232.054 | 227.240 | -4.814 |
| Q025 | 106.692, 94.047 | 94.274, 92.849 | 104.234 | 94.094 | -10.140 |
| Q116 | 117.604, 115.324 | 119.720, 120.195 | 116.464 | 119.957 | +3.493 |
| Q099 | 3.432, 6.922 | 5.760, 8.680 | 4.733 | 7.898 | +3.165 |

Q006 and Q025 are consistent with a modest improvement, while Q025's baseline
blocks differ substantially. Q116 and the index-only Q099 do not improve in
this run; Q099 varies by several milliseconds within and between blocks even
though it does no source reads. These samples do not establish an attributable
latency regression or a precise speedup. Candidate, open and parse counts are
unchanged within each case. The deterministic reduction is one unnecessary
seek per fresh zero-offset singleton source read, with tests covering reused
handles, adjacent and backward reads, coalesced windows and errors. The
benchmark's ordinary counters do not count seeks.
