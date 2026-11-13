# Boolean field predicates: measured results

Task: `boolean-field-predicates` (Plan 2). Acceptance report, 2026-11-13.

## Implementation and semantics

Base: completed Plan 1 at `c54afc3eb1cdc496734bfb30bcc9a3ea38988b78`. The predicate implementation was measured at `a7a31feb5da625008c8e0e979de32bdcb64a80cd`; a later repair at `d666524f7f` restored exact null postings for indexed fields proven scalar by inference. The repair changes no build code or permanent index payload. The final 206-case run below uses the repaired revision.

The shared source evaluator aggregates selected logical values existentially, then compares the Boolean predicate result. `:?` tests nonemptiness: zero, false and whitespace strings are nonempty; null, empty strings and empty containers are empty. `:exists` tests any selection, whereas `:null` tests any selected null; a missing field is neither. The type modifiers test the selected logical value, so an array is an array and an explicitly selected element has its own type. Equality and inequality accept Boolean operands. Binder/parser, CLI, TUI and desktop completion/filter-builder use these rules. Complex unsupported builder expressions remain in the text editor. The desktop predicate selector is a standard menu.

Existing structural null postings represent a singleton-null selection. On a multivalued index they cannot prove an aggregate `:null` predicate, so estimate, EXPLAIN and execution take source fallback. An index proven scalar retains exact null postings even when schema-free binding conservatively marks a term multivalued. Presence uses its existing exact posting where safe. There is no permanent state bitmap or additional construction pass; Plan 3 owns predicate acceleration.

## Construction and memory

Three alternating fresh-process warm Musenalm pairs compared Plan 1 with Plan 2 on the same absolute 148,486-file corpus, schema, ASCII alphabet, 32 workers, incremental path, release build and warm filesystem cache. The [raw pairs and summary](../../../bench/results/boolean-field-predicates/construction-pairs/summary.json) retain every command, sample and manifest. These are true process high-water RSS measurements; the query runner's cumulative RSS is not substituted for them. Each side made exactly 148,486 source reads and parses per trial. No material build change required seven pairs under the Plan 2 task rule.

| Measure | Plan 1 median | Plan 2 median | Change |
| --- | ---: | ---: | ---: |
| Full build | 897.922 ms | 921.155 ms | +2.59% (below 5% investigation threshold) |
| Fresh-process peak RSS | 573.559 MB | 576.315 MB | +0.48% |
| RSS after build | 564.130 MB | 566.354 MB | +0.39% |
| Estimated live index | 69.966 MB | 69.966 MB | 0% |
| Construction reads/parses | 148,486 | 148,486 | equal |

Against original master, Plan 1's seven-pair medians were 973.344 ms build, 536.842 MB peak RSS and 69.663 MB live index. Plan 2's three-pair medians are about 5.36% faster build, 7.35% higher peak RSS and 0.44% higher live index. These cross-campaign percentages are indicative; the directly matched acceptance comparison is Plan 1 versus Plan 2 above.

## Query correctness and latency

The [predecessor-matched comparison](../../../bench/results/boolean-field-predicates/vs-plan1/compatibility.json) used the unchanged 194-case workload at `a7a31feb5d`, four profiles (ASCII/Unicode × cache off/warm), five timed repetitions, same corpus/schema fingerprints, and Plan 1's final query revision `ad542f6e9ad3ede9fca0e2235df61476ec721749`. All 776 status/count rows and normalized result digests matched; there were zero source-open increases. Cache-off sums of per-case medians were 983.340→983.239 ms (ASCII) and 980.562→1001.152 ms (Unicode). Warm sums were 2.954→3.028 ms (ASCII) and 2.789→2.721 ms (Unicode). Sums are descriptive, not a substitute for per-query inspection.

Seven individual full-suite cache-off rows exceeded both 10% and 0.5 ms. [Three alternating seven-repetition matched pairs](../../../bench/results/boolean-field-predicates/focused-pairs/campaign.json) did not reproduce six of them. Q004 ASCII remained +11.2% and +1.428 ms in that focused campaign; [five further isolated alternating 15-repetition pairs](../../../bench/results/boolean-field-predicates/q004-isolated/campaign.json) measured 9.284 ms Plan 1 versus 9.099 ms Plan 2, with equal 7,766 source opens and 2,995 matches. The apparent regression was not repeatable. All 194 per-case rows and query plans remain in the archived comparison.

The repaired final run's one-process timings were broadly slower on unrelated cases (ASCII cache-off sum 1,162.100 ms versus the earlier Plan 1 run's 983.340 ms). Its correctness and counters passed, but this cross-time latency comparison was not used as an acceptance claim. [Three alternating full 194-case cache-off pairs](../../../bench/results/boolean-field-predicates/final-paired-query/summary.json) on the final code measured per-case median sums of 1,116.560→979.961 ms ASCII and 1,122.081→989.799 ms Unicode. The only repeated-suite threshold outlier was Q003 ASCII (6.194→7.497 ms); [five isolated alternating 15-repetition pairs](../../../bench/results/boolean-field-predicates/q003-isolated/campaign.json) measured 4.054→3.955 ms with equal source opens and matches. Thus the broad slowdown and Q003 outlier did not repeat under matched focused conditions. These sums describe the same case mix but do not conceal the archived per-case table.

The repaired final revision ran [all 206 cases](../../../bench/results/boolean-field-predicates/final-query/query-results.csv) in four profiles with five repetitions: 824/824 statuses passed. Its first 194 resolved queries, counts, normalized digests and source-open counts [match Plan 1](../../../bench/results/boolean-field-predicates/final-vs-plan1/compatibility.json). The 12 added predicate cases Q195–Q206 were also measured in an earlier focused three-repetition run: [48/48 statuses passed](../../../bench/results/boolean-field-predicates/predicates/query-results.csv). Cache-off ASCII examples below expose the interim source-fallback cost; zero source opens for Q197 reflects the old exact `:exists` posting, not new type-predicate acceleration.

| Case / predicate | Matches | Source opens | Median |
| --- | ---: | ---: | ---: |
| Q195 `annotation:? = true` (unindexed) | 0 | 4,010 | 3.323 ms |
| Q196 `annotation:string = true` (unindexed) | 4,010 | 4,010 | 3.678 ms |
| Q197 `year:exists = true` | 4,928 | 0 | 0.021 ms |
| Q198 `year:number = true` | 4,928 | 4,928 | 3.113 ms |
| Q199 `year:? = false` | 0 | 4,928 | 2.394 ms |
| Q202 `contributors:array = true` | 133,948 | 133,948 | 99.689 ms |
| Q203 `contributors[*].type:null = true` | 0 | 133,948 | 72.663 ms |
| Q204 `contributors:? = false` | 63,520 | 133,948 | 83.693 ms |

The omitted Q200–Q201 and Q205–Q206 rows are retained in the CSV and also use source fallback. `contributors` scans are intentionally expensive until Plan 3 records structural facts for admitted indexed fields. No benchmark labels those source evaluations as index-accelerated.

## Verification and clients

At the repaired code revision, `cargo test --workspace --all-targets --locked` passed 914 tests, 30 ignored, zero failed. The added tests cover the source truth table, multivalued null safety, inferred scalar exactness, incomplete views, parser/parameters, CLI process behavior, and desktop filter-builder/completion round trips. `cargo clippy --workspace --all-targets --locked` exited 0 with 215 nonfatal warning locations, exactly the same count and file distribution as the Plan 1 predecessor on the current toolchain. `cargo fmt --all --check` and `python3 scripts/format-rust-spacing.py --check` still fail on inherited repository style drift: rustfmt flags 99 predecessor files versus 95 candidate files, with zero candidate-only files. `git diff --check` passed. The release benchmark built successfully.

[Native desktop screenshots](../../../bench/results/boolean-field-predicates/2026-11-13-client-smoke/README.md) show the builder selecting `Nonempty` and serializing `x:?`, then `x:? = true` returning text and zero, and `x:? = false` returning missing, null and empty string in a five-file fixture. The app exited through the repository's task-owned desktop cleanup. CLI process tests and TUI renderer tests passed. The desktop behavior was checked on the native headless Wayland session, not a browser mock.

## Remaining limits

New type/nonempty predicates generally use source fallback, especially on multivalued fields; their measured cost is the explicit Plan 3 acceleration target. The predecessor's larger incremental snapshot and early-view semantics remain. No in-scope correctness failure or repeatable query regression remains open. This branch is not merged into master.
