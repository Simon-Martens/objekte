# Cumulative snapshot and memory reassessment after Plan 1 repair

**Cumulative branch:** `integration/indexed-field-structural-postings`
**Before:** `7a9dee532e75bf167b6e4eabfd52277c79bb4df1`
**Repaired merge:** `23d38d5e3fe5559879fc26a0e8bf0a803f2e51bb`
**Original master reference:** `7a6b96684e1fbb849eab7cc20185b942159ed189`
**Worktree:** `/home/simon/source/objekte-integration-indexed-field-structural-postings`

The canonical metadata, derived-lookup, and empty-base repairs were implemented on Plan 1 and merged through Plan 2 into Plan 3. The only merge conflict was the snapshot version: cumulative Plan 3 now uses 27/28, rejecting both old Plan 1 (23/24) and old Plan 3 (25/26) layouts. A newly generated 26-version Plan 3 lifecycle snapshot failed in the repaired loader with `invalid index cache: incompatible snapshot version`; repaired snapshots restored successfully in every paired trial. Plan 3's indexed structural facts remain. [Plan 1's report](plan_1_snapshot_persistence_repair_results.md) contains the exact defect attribution and implementation detail.

## Paired cumulative results

Seven alternating fresh-process pairs compared the before and repaired Plan 3 commits. Every pair ran three distinct workloads: incremental construction alone, construction plus full explicit save, and fresh restore from that mode's newly saved snapshot. All used the same 148,486-file Musenalm extended corpus, ASCII bigrams, 32 workers, release rustc 1.99.1, `--profile`, warm page cache, and checked queries. Timing trials had byte diagnostics off. The identical benchmark-only save-call/restore harness was present in both binaries. The [raw runs](../../../bench/results/indexed-field-structural-postings/2026-11-13-snapshot-repair-profile-pairs/runs.json), [summary with quartiles and paired differences](../../../bench/results/indexed-field-structural-postings/2026-11-13-snapshot-repair-profile-pairs/summary.json), and [method](../../../bench/results/indexed-field-structural-postings/2026-11-13-snapshot-repair-profile-pairs/README.md) are committed. Values below are medians [inclusive Q1–Q3], ms or decimal MB.

| Measure | Before | Repaired |
| --- | ---: | ---: |
| Catalogue readiness, ms | 244.54 [226.99–264.91] | 240.18 [238.60–246.89] |
| First searchable result, ms | 319.17 [313.96–344.26] | 317.74 [315.68–328.55] |
| Full construction, ms | 990.39 [965.81–993.11] | 963.23 [953.47–986.90] |
| Construction peak RSS, MB | 575.95 [568.88–576.29] | 569.50 [562.33–570.85] |
| Post-build RSS, MB | 565.63 [559.42–566.74] | 559.29 [552.72–560.54] |
| Complete external save call, ms | 389.39 [388.66–398.61] | 306.80 [302.20–311.66] |
| Inner writer timer, ms | 207.16 [205.06–211.95] | 163.03 [160.80–164.73] |
| Lifecycle snapshot size, MB | 179.58 | 132.71 |
| Build + save peak RSS, MB | 783.03 [778.58–786.00] | 720.42 [712.63–722.63] |
| Post-save RSS, MB | 611.55 [606.53–613.11] | 608.45 [600.35–610.49] |
| Fresh load call, ms | 956.23 [941.37–958.64] | 757.91 [750.92–764.32] |
| Fresh restore peak RSS, MB | 493.39 [493.21–493.51] | 406.60 [406.24–406.72] |
| Post-load RSS, MB | 357.84 [357.68–357.85] | 329.08 [329.04–329.16] |

Snapshot reduction is **46,865,024 bytes (26.10%)**, from 179,575,108 to 132,710,084. Repaired Plan 3 remains **1,287,580 bytes larger** than repaired Plan 1 (131,422,504), preserving structural postings/facts, and **3,570,626 bytes above** the original master's nonincremental 129,139,458-byte reference. The original-master fresh reference had 534.37 MB construction and 541.26 MB build/save peak RSS; the previously published seven-run master median save peak was 543.77 MB. Master and incremental construction/snapshot models differ, so this is a reference rather than a paired effect estimate.

Construction still made exactly 148,486 source reads/parses in all before and repaired build-only trials. Every fresh restore reported zero source reads/build parses, and all checked query digests matched. The full save-call timer begins before lifecycle capture and segment encoding; the inner writer timer does not. High-water RSS is process cumulative and was never reset or subtracted as an allocation. The paired full-construction median difference is noisy (the median of paired after-minus-before values is +13.57 ms even though the marginal after median is lower), and both views are within the existing build limits. No additional parse pass was introduced.

## Query and structural correctness

The complete before/after query campaign ran **220 cases × four profiles = 880 rows per revision**, with three repetitions per row, ASCII/Unicode and cache off/warm. All 880 repaired statuses were `ok`; normalized result digests, match counts, returned rows, files opened, bytes read, parses, exact posting matches, scans, and residual evaluations matched the before revision in every row. The [comparison](../../../bench/results/indexed-field-structural-postings/2026-11-13-snapshot-repair-profile-pairs/query_comparison.json) and full raw CSVs/plans/digests are archived. The structural reference verifier independently returned equal counts/page digests with **zero source opens, bytes, parses, and scans** before and after for Q115–Q118 and Q120, covering presence/null/empty/type/nested structural paths. Core unit tests also cover `:?` and the new Boolean/type modifiers through snapshot restore.

Four one-process cache-off latency rows initially exceeded both 10% and 0.5 ms. In three alternating focused seven-repetition pairs, Q141 was +9.3%, Q177 +1.1%, and Q209 +2.8%; Q214 ASCII remained +17.1% and +0.512 ms. Five further alternating isolated 15-repetition pairs measured Q214 at **3.066 → 3.110 ms (+1.4%, +0.043 ms)** with zero source opens on both sides. Thus no threshold regression repeated under isolation. All focused raw trials are archived beside the full campaign.

## Remaining memory cost and review

One repaired opt-in attribution reconciled all **132,710,084** encoded bytes exactly. Its segment payloads total **104,163,001 bytes**. These buffers are retained in `LifecycleCacheState` during outer streaming serialization. Restore deserializes the entire lifecycle section and keeps encoded buffers while decoding segment structures. The median repaired save peak remains **720.42 MB**, 150.92 MB above the repaired construction-only peak and 179.16 MB above the one-run original-master save reference. Those are separate-process peak comparisons, not a measured allocation. The repaired restore peak is 406.60 MB versus 329.08 MB post-load RSS within its process. The retained buffers are a material, bounded follow-up recorded in [the performance issue](../issues/perf/lifecycle_snapshot_segment_buffer_peak.md). The representation repairs are complete, but this residual save peak is part of the user's merge review; **no merge into master was performed**.

`cargo test --locked --workspace --quiet` passed on the cumulative merge (core library 534 passed, 16 ignored; other workspace packages passed). Plan 2's merged core library passed 528, 16 ignored. The normal desktop/CLI save and reopen paths are exercised by existing workspace tests; the separate desktop query/completion repair adds its own native smoke and focused tests. No user data was altered by the snapshot comparisons.
