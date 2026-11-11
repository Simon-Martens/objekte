# Borrowed segment verification benchmark rerun

Date: 11 November 2026. This rerun measures the follow-up task in [`objekte_borrowed_segment_verification_follow_up_specification.md`](../done/objekte_borrowed_segment_verification_follow_up_specification.md). It makes no code changes.

## Revisions and method

- Predecessor baseline: `5166876c7228f4fe2f8f064861cdc8a88275ad18` (`master` before the follow-up).
- Follow-up implementation: `f872e8d750fd2f7f83994a147394b84dcbf7d21e` on `integration/borrowed-segment-verification`; implementation commits `1f906cb1be16`, `c3b01ab22d18`, and `47931f0cdfef`.
- Both release binaries were built with `cargo build --release --locked -p objekte-search-core --example bench_candidate_verification --example bench_musenalm`, using Rust/Cargo 1.99.1 on an AMD Ryzen 7 5700G (8 cores, 16 threads). The focused benchmark harness was byte-identical in both worktrees (SHA-256 `9ef7647d7dfed88f048407c558e6b8ad90527cce8c6bc0a9bec77cde3fb9312e`).
- Runs were serial on this computer with four query workers. Focused runs used locator output and evidence, one warm-up, seven timed repetitions, a separate untimed allocation pass, and no verified-result cache. The long escaped fixture also had three serial baseline/final pairs with 21 timed repetitions each; pair order alternated. The full Musenalm suite used three repetitions per profile, with cache off or 64 MiB warm. No other benchmark ran concurrently.
- The five regenerated synthetic fixtures match all archived corpus SHA-256 digests in [`fixtures/`](../../../bench/results/borrowed-segment-verification/fixtures/). The checked-in `examples/musenalm` Git tree is identical at both revisions (`ed68fbad86b9cfd1c88c7874dbeb42ba99b4a6ed`). The benchmark's corpus fingerprint differs because it includes checkout file modification times; its schema and workload path hashes include different absolute worktree paths. `queries.json` and `schema-manifest.json` are byte-identical between revisions.

Exact command arrays, working directories, raw timing samples, allocation counts, and work counters are in the [rerun archive](../../../bench/results/borrowed-segment-verification/rerun-2026-11-11/README.md). Absolute times here are from this computer and should be compared only within these paired runs.

## Correctness and focused measurements

All 80 focused baseline/follow-up query pairs matched hit count, returned count, ordered membership digest, candidate count, source bytes read, file opens, and parse count. The 36 additional non-focused rows on plain and escaped fixtures also matched membership. Of those, the 24 public in-memory owned extraction/evaluation rows had identical allocation counts and allocated bytes at both revisions; their 12 repository-search rows reflect the optimized path and can differ in allocations. The complete [focused comparison CSV](../../../bench/results/borrowed-segment-verification/rerun-2026-11-11/focused-comparison.csv) includes minimum, median, maximum, allocations, and allocated bytes for every pair.

Selected seven-repetition complete-query medians and gross allocation counts (including source buffers, output, evidence, and bookkeeping):

| Fixture and query | Baseline → follow-up median | Baseline → follow-up allocations |
| --- | ---: | ---: |
| Plain JSON, required-value identity | 3.394 → 3.311 ms | 54,652 → 38,272 |
| Plain JSON, source combined FTS/contains | 3.811 → 3.765 ms | 58,999 → 35,794 |
| Duplicate-heavy decoded JSON, source combined | 16.717 → 3.167 ms | 957,077 → 11,753 |
| Field duplicates plus 128 scalar values, scoped FTS | 5.391 → 1.228 ms | 287,225 → 6,101 |
| Long escaped decoded JSON, source combined | 15.847 → 14.950 ms | 45,946 → 15,211 |
| Long escaped decoded JSON, scoped FTS | 12.330 → 16.764 ms | 43,889 → 15,203 |

The long fixture has 1,024 records with 8 KiB logical text and JSON escapes; 683 records were candidates for each source-only query. The exact queries are unscoped literal FTS `good` together with FILTER `text ~ 'good'` (`source_combined`), and literal FTS `good` scoped to field `text` (`scoped_duplicates`). Both use locator output with evidence.

The longer recheck shows considerable timing drift. Ratios below are follow-up/baseline medians for the same serial pair; values over 1 are slower:

| 21-repetition pair | Combined FTS/contains | Scoped FTS | High-hit logical output |
| --- | ---: | ---: | ---: |
| 1, baseline then follow-up | 12.688 → 15.232 ms (1.200×) | 11.434 → 12.555 ms (1.098×) | 14.358 → 15.963 ms (1.112×) |
| 2, follow-up then baseline | 12.805 → 11.950 ms (0.933×) | 11.813 → 11.850 ms (1.003×) | 15.213 → 14.978 ms (0.985×) |
| 3, baseline then follow-up | 12.137 → 11.563 ms (0.953×) | 11.694 → 12.484 ms (1.068×) | 15.141 → 14.786 ms (0.977×) |

The earlier report's repeatable 5–14% long-escape slowdown is therefore **not reproduced consistently for the combined query** in this rerun. Scoped FTS is slower in two of the three longer pairs by 6.8–9.8%, and essentially equal in the other. This is evidence of a possible small scoped cost, with run-to-run variability too large to claim a stable absolute penalty from this run alone. Allocation reductions are stable across every pair. No code was rerouted to the allocating path.

## Full Musenalm suite

Both revisions passed all 194 queries in ASCII and Unicode with cache off and warm (776 case/profile rows per revision). All 776 paired rows have identical status, result and returned counts, candidates, source opens and bytes, parse counts, scanned records, residual evaluations, posting counts, and verified-result hit counts. Both runs report zero failures. The benchmark's own query oracle checks passed. Median of per-query follow-up/baseline timing ratios:

| Profile | Cache | Median ratio |
| --- | --- | ---: |
| ASCII | off | 0.986× |
| Unicode | off | 0.937× |
| ASCII | 64 MiB warm | 0.938× |
| Unicode | 64 MiB warm | 0.934× |

Individual short-query ratios are noisy; these medians are descriptive, not a wall-time guarantee. Build control results used the same 148,486 parsed records per alphabet and identical estimated index sizes (68,963,924 ASCII; 71,872,324 Unicode bytes). Baseline → follow-up build times were 2.656 → 2.828 s and 2.527 → 2.671 s in the cache-off runs, then 2.948 → 2.775 s and 2.727 → 2.755 s in the warm runs (ASCII, Unicode respectively). The changes in both directions do not show a repeatable build regression.

## Limits

This rerun did not change code or repeat the full workspace tests; `cargo test --workspace --locked` passed on this same implementation immediately before the benchmark request. The older [implementation and performance report](borrowed_segment_verification_results.md) remains the record of the original campaign. The paired timing samples here were not CPU-pinned and visibly vary between rounds. Raw files are retained so the user can inspect dispersion and rerun particular queries.
