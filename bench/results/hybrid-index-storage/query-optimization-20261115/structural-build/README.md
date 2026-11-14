# Structural field admission: seven paired build and flush trials

Base source: `8ca2851ab8`; candidate: the subtask worktree after adding persistent `values_indexed` and structural facts for every declared field. Both used `examples/musenalm` in the integration worktree, the incremental backend, ASCII, 32 construction workers, alternating order and fresh processes. The exact commands, binary SHA-256 values, and unrounded samples are in `construction-trials.json` and `flush-trials.json`. Candidate binaries were built from the reviewed worktree before the final documentation/test edits; those edits do not change build or storage behavior.

| Median measure | Base | Candidate | Ratio |
| --- | ---: | ---: | ---: |
| Complete construction | 1,117.31 ms | 1,162.86 ms | 1.041 |
| Retained RSS after build | 592.96 MB | 607.31 MB | 1.024 |
| Peak RSS after build | 605.47 MB | 620.68 MB | 1.025 |
| Estimated index bytes | 11,918,518 | 14,229,308 | 1.194 |
| First flush | 297.76 ms | 284.31 ms | 0.955 |
| Logical stored bytes | 128,312,322 | 131,082,617 | 1.022 |
| Allocated stored bytes | 128,335,872 | 131,108,864 | 1.022 |

The structural facts add 2,310,790 estimated bytes and 2,770,295 logical stored bytes. The resulting 2.4% retained RSS and 2.2% store-byte growth meet the plan's 5% limits. Construction is 4.1% slower, beyond its 2% target, or about 46 ms per build. The added work traverses every declared field while recording presence and structural state; it enables exact state queries over fields that were previously scanned. The cost is retained and reported rather than narrowing structural coverage or changing query semantics. Flush's apparent improvement is small relative to process noise and is not treated as a proven speedup.

`FORMAT_VERSION` changed from 1 to 2 for the new field-binding flag, and `MODEL_VERSION` changed from 11 to 12 so the configuration identity creates a new store. The older `:length` and empty-string shortcuts remain for admitted unindexed fields. Auto-index-prohibited fields receive structural facts but retain their prior lack of those shortcuts. Segment and snapshot round trips check the flag; file replacement, move, deletion and addition tests check unindexed structural facts through mutation.
