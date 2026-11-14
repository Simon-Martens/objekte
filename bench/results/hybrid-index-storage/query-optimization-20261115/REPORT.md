# Query scanner and posting plans: final campaign

## Provenance and method

The task branch is `task/hybrid-index-storage/query-scanner-plans`, based on integration commit `8ca2851ab8`. The final candidate is `aaf732dd93` (release executable SHA-256 `027c12fabecbc113697abb29f9707e1424d13a62722736120f9ab90cd6946e7a`). The baseline release executable was built from `8ca2851ab8` (SHA-256 `9baebc3cbc615fd4528cbea7b9716ddbfcdf94ce2dc069ad8c85eb990695a358`). The benchmark manifest's `code_revision` for the baseline records the *runtime checkout* (`aaf732dd93`), not that baseline binary's source revision. Rust was `rustc 1.99.1 (58cf8f9c3 2026-10-13)`; Cargo.lock SHA-256 was `e8915ae2f6dc40433bf346ac0060f4a58a130ae17688801f24da0b744ed8d200`.

All runs used the existing full Musenalm fixture at `/home/simon/source/objekte-integration-hybrid-index-storage/examples/musenalm`, the extended incremental workload, ASCII and Unicode profiles, 32 query workers, a 16 MiB prepared-plan cache, and a zero-byte verified-result cache. The schema fingerprint is `2edaa7c59a2f5d7aad8b8b6cb6065420a23c0a12db3a6b3133eb5bfdbbacf819`, workload fingerprint `4ce8645bc154a809c73b365aa3fd003443c3b9584b4f0521a1a21ff6d55bf829`, and corpus fingerprint `ad517d15e3216bfbdb569906d14a1717271daaf27a8d58e47e55e1e63e5db868`. Fingerprints match across modes.

The full campaign command shape was `bench_musenalm --root <fixture> --extended --incremental --profiles ascii-off,unicode-off --repetitions 5 --verified-cache-bytes 0 --prepared-cache-bytes 16777216 --query-workers 32 --result-dir <mode>`. Memory runs constructed from source. Fully resident and warm on-demand runs added `--storage custom <store>`, and the former added `--fully-resident`. The exact commands, binary hashes, manifests, CSV summaries, plan traces, and raw `measurements.jsonl` are in `full/{baseline,memory,resident,ondemand}/`; `scripts/summarize.py` regenerates `summary.json` and validates coverage, fingerprints, and digests. The candidate stored runs opened its version 2 store source-free: zero source reads and build parses. The older baseline store was used separately for the paired open measurements below.

The focused tail used `scripts/tail_abba.py`: A/B/B/A fresh-process order, 30 repetitions per run, 12 cases × two profiles, with 60 baseline and 60 candidate samples for each case/profile. Its commands, raw samples, and summary are in `tail-abba/`. Phase 0 used the *same* baseline binary in both A and B; its pair geomeans were 0.985 and 0.924. This bounds confidence in small timing differences. `perf stat` was available for process-level counters; `perf record -g` was denied by host policy. See `phase0/`.

## Full campaign

All 440 case/profile rows succeeded in each mode. Result digests, which include counts, returned hits, and matched-field evidence, are identical among baseline, candidate memory, candidate fully resident, and candidate on-demand. The table reports candidate/baseline geomean ratios over 220 cases per profile; lower is faster. The baseline denominator is the baseline **memory** binary for every mode, as specified in the task. Stored-mode ratios therefore include residency and format differences, and are not a controlled before/after estimate for stored query execution.

| Mode | ASCII median | ASCII p95 | Unicode median | Unicode p95 |
| --- | ---: | ---: | ---: | ---: |
| Memory | 0.789 | 0.790 | 0.747 | 0.776 |
| Fully resident | 0.828 | 0.860 | 0.755 | 0.833 |
| Warm on-demand | 0.803 | 0.823 | 0.778 | 0.846 |

The large geomean improvement is dominated by Q187/Q202/Q203/Q204, whose full scans became exact postings. Excluding those four cases, the memory median geomean is 0.893 for ASCII and 0.849 for Unicode. All aggregate median and p95 ratios satisfy the task's 1.05/1.10 ceilings.

### Selected cases

The following full-campaign medians are milliseconds. The three candidate columns are directly measured, not inferred from ratios. Some stored scanner cases exceed the memory baseline; the task's prescribed cross-residency comparison cannot isolate the query change there. The interleaved memory comparison below is the controlled evidence for those cases.

| Case | Profile | Baseline memory | Candidate memory | Candidate resident | Candidate on-demand |
| --- | --- | ---: | ---: | ---: | ---: |
| Q006 | ascii-off | 142.511 | 123.914 | 142.078 | 141.017 |
| Q006 | unicode-off | 135.343 | 129.154 | 150.294 | 154.420 |
| Q007 | ascii-off | 170.857 | 151.068 | 179.933 | 175.352 |
| Q007 | unicode-off | 173.325 | 154.339 | 175.732 | 177.349 |
| Q025 | ascii-off | 66.758 | 61.010 | 75.632 | 73.933 |
| Q025 | unicode-off | 63.863 | 63.738 | 72.149 | 74.088 |
| Q026 | ascii-off | 68.473 | 64.532 | 77.474 | 74.298 |
| Q026 | unicode-off | 70.255 | 64.562 | 76.936 | 80.286 |
| Q167 | ascii-off | 3.272 | 3.226 | 3.422 | 5.901 |
| Q167 | unicode-off | 3.415 | 3.295 | 3.273 | 4.356 |
| Q174 | ascii-off | 19.627 | 19.512 | 19.178 | 19.049 |
| Q174 | unicode-off | 20.524 | 20.081 | 20.034 | 18.886 |
| Q175 | ascii-off | 3.304 | 3.622 | 4.104 | 3.336 |
| Q175 | unicode-off | 3.259 | 2.804 | 4.162 | 4.098 |
| Q177 | ascii-off | 2.763 | 2.578 | 2.890 | 2.779 |
| Q177 | unicode-off | 2.906 | 2.914 | 2.927 | 3.328 |
| Q187 | ascii-off | 111.818 | 0.126 | 0.126 | 0.127 |
| Q187 | unicode-off | 114.314 | 0.135 | 0.137 | 0.219 |
| Q202 | ascii-off | 104.253 | 0.201 | 0.128 | 0.124 |
| Q202 | unicode-off | 96.987 | 0.122 | 0.120 | 0.115 |
| Q203 | ascii-off | 79.867 | 0.022 | 0.014 | 0.014 |
| Q203 | unicode-off | 81.965 | 0.014 | 0.014 | 0.014 |
| Q204 | ascii-off | 95.807 | 0.154 | 0.098 | 0.097 |
| Q204 | unicode-off | 93.191 | 0.096 | 0.096 | 0.100 |

### Interleaved tail

Ratios below are pooled candidate/baseline medians from the A/B/B/A tail. The complete per-run measurements and digest checks are in `tail-abba/summary.json` and its four run directories. Every tail result digest matches.

| Case | ASCII baseline → candidate (ms) | ASCII ratio | Unicode baseline → candidate (ms) | Unicode ratio |
| --- | ---: | ---: | ---: | ---: |
| Q006 | 137.526 → 127.995 | 0.931 | 138.041 → 128.824 | 0.933 |
| Q007 | 171.198 → 153.996 | 0.900 | 170.272 → 154.775 | 0.909 |
| Q025 | 68.106 → 62.659 | 0.920 | 67.485 → 63.746 | 0.945 |
| Q026 | 70.238 → 66.799 | 0.951 | 72.630 → 66.813 | 0.920 |
| Q167 | 3.603 → 3.085 | 0.856 | 3.557 → 3.491 | 0.982 |
| Q174 | 19.596 → 18.917 | 0.965 | 20.353 → 19.537 | 0.960 |
| Q175 | 3.166 → 3.038 | 0.960 | 3.049 → 2.776 | 0.911 |
| Q177 | 2.804 → 2.598 | 0.927 | 2.739 → 2.651 | 0.968 |
| Q187 | 114.506 → 0.121 | 0.001 | 115.252 → 0.125 | 0.001 |
| Q202 | 105.934 → 0.124 | 0.001 | 105.335 → 0.123 | 0.001 |
| Q203 | 86.337 → 0.014 | 0.00017 | 85.768 → 0.014 | 0.00016 |
| Q204 | 98.473 → 0.089 | 0.001 | 98.665 → 0.089 | 0.001 |

Q006, Q025, and Q026 do not consistently reach the 10% scanner target; Q007 is at or near it. The improvement comes from a borrowed-source scanner path, deferred evidence for page winners, membership-only FTS checks, and grouped consecutive file IDs with a sorted fallback. The scanner still parses the same candidate records. Further change would need a separate profiling pass; the phase 0 control shows that small percentages are sensitive to run order and host noise. The focused preliminary scanner and decoded-posting cache checkpoints are in `scanner-checkpoint/` and `clock-checkpoint/`.

## Plans, correctness, and work counters

Q187 and Q202–Q204 now use exact posting plans with `filter_exact=true`, no record parses, no source reads, and no residual evaluations in the final plan trace. Q187 uses an exact presence test for the proven identical-field equality case. Q202/Q204 use structural state facts for declared unindexed `contributors`; Q203 uses exact null state across the declared `[*]` step. Their full-campaign medians are all below 0.22 ms, well within the 10/5 ms targets. Inferred/dynamic paths and unproven explicit-array or quantifier combinations remain scanner evaluated. The differential fixtures cover missing/null/empty selections, nested arrays, declared unindexed fields, direct and segmented access, reopened stores, and mutation paths. Full-campaign evidence parity additionally checks the real corpus.

Q174's segmented EXPLAIN now estimates 12,554 FTS bigram candidates instead of treating all 133,948 records as candidates. The filter still requires verification and scans 9,398 records after intersecting the available restrictions. Its interleaved memory ratio is about 0.96 in both profiles; the change chiefly repairs plan quality and accounting. Q175 now reports `relation_reverse_postings_visited=2,613` for reverse lists and `relation_reverse_members_visited=95,153` for memberships, with zero source reads and the same result digest. The two counters have distinct, stable units in direct and segmented execution.

The decoded-posting cache uses read-lock hits and an atomic recent-use bit with bounded clock eviction, without a per-hit recency-map mutation. The per-dictionary validation mutex was already removed by the earlier maintenance series. Cache keys still include descriptor, domain, and proof; uncached externally loaded postings remain checked. Existing corruption and concurrency tests passed. The focused cache checkpoint does not establish a speedup: Q167 0.988, Q175 1.001, and Q177 1.038 ratios in sequential ASCII runs. Their interleaved results are in the table above.

## Construction, storage, and source-free opens

Seven alternating fresh-process build/flush pairs compare `8ca2851ab8` with the structural-posting candidate. The raw samples and exact commands are in `structural-build/`.

| Median metric, ASCII | Baseline | Candidate | Ratio |
| --- | ---: | ---: | ---: |
| Full construction | 1,117.31 ms | 1,162.86 ms | 1.041 |
| Retained RSS | 592.96 MB | 607.31 MB | 1.024 |
| Peak RSS | 605.47 MB | 620.68 MB | 1.025 |
| Estimated index bytes | 11,918,518 | 14,229,308 | 1.194 |
| First flush | 297.76 ms | 284.31 ms | 0.955 |
| Logical stored bytes | 128,312,322 | 131,082,617 | 1.022 |

The structural facts add 2,310,790 estimated index bytes and 2,770,295 logical stored bytes in ASCII. Unicode candidate stored bytes are 133,930,453 versus baseline 131,160,158, a 2.1% increase. The final campaign's single-run ASCII construction ratio is 1.021 and Unicode is 0.992, but the seven alternating pairs are stronger evidence: **construction is 4.1% slower**, above the 2% target. The added traversal records state for every declared field. Retained/peak RSS and stored-byte growth stay under their 5% limits. The new persisted `values_indexed` flag required segment format version 2 and model version 12, so old stores are rebuilt under the new configuration identity.

`scripts/open_pairs.py` separately measured seven alternating source-free opens of the baseline format 1 store and candidate format 2 store with Q167 as a fixed correctness probe. All opens had zero source reads and build parses and matching digests. Medians:

| Mode | ASCII baseline → candidate | Ratio | Unicode baseline → candidate | Ratio |
| --- | ---: | ---: | ---: | ---: |
| Fully resident | 445.4 → 469.2 ms | 1.053 | 474.1 → 501.0 ms | 1.057 |
| Warm on-demand | 381.9 → 396.9 ms | 1.039 | 390.7 → 407.6 ms | 1.043 |

This 4–6% reopen cost is a real penalty in these paired runs. The expanded structural store and its validation contribute extra work; the measurement does not isolate their individual shares. `open-pairs.json` preserves every sample and exact command.

## Validation and disposition

`cargo fmt --all`, `cargo test --workspace --locked --quiet`, and `cargo clippy --workspace --all-targets --locked --quiet` passed on the candidate after the final code edits. The repository-wide `scripts/format-rust-spacing.py --check` still reports inherited formatting drift outside this subtask; unrelated formatter edits were restored. Structural segment/snapshot round trips, mutations, differential evaluator checks, corruption handling, and cache concurrency tests are included in the passing suite. The combined integration worktree is tested again after merge.

1. Structural postings and exact state routing: **implemented and verified** for declared fields, including non-value-indexed fields and proven occurrence-existential explicit-array steps.
2. Identical-field equality: **implemented and verified** as exact raw path presence; inequality and other unproven cases stay on the evaluator.
3. Scanner allocation/evidence reduction: **implemented, parity verified, and improved in memory interleaving**, with some focused gains below 10% and stored-mode cross-residency slowdowns recorded above.
4. Posting proof/hot path: **implemented and correctness verified**; focused timing is inconclusive at millisecond scale.
5. FTS estimate and relation accounting: **implemented and verified**; Q174's 12,554 candidate estimate and distinct list/member counters are recorded in final plans.

Cold-index and constrained-memory behavior have not been measured; all stored-query runs use warm stores. Phase 0 noise limits claims about small timing differences. The construction and reopen penalties exceed their respective desired ceilings and remain visible tradeoffs for the large exact-query gains. Per the user's instruction, these measured ratios are documented and the task proceeds without further micro-optimization.
