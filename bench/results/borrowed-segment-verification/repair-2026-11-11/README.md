# Borrowed-segment verification repair, 11 November 2026

## Revisions and scope

| Label | Commit | Role |
| --- | --- | --- |
| Predecessor | `5166876c7228f4fe2f8f064861cdc8a88275ad18` | Completed implementation before the borrowed-segment work. |
| Reviewed | `47931f0cdfefb860b0a0cac17ae06e068daf8a1e` | Borrowed-segment implementation inspected in the repair task. |
| Starting integration | `dfde3511f98167838df903a78bc579d8e91ded06` | Integration head when the repair began; includes intervening commits after the reviewed head `f872e8d750fd2f7f83994a147394b84dcbf7d21e`. |
| Measured repair | `49ac3f503ac63f8303f85122a1ec4f0043949cda` | Production code used in the complete full-suite, focused, and flagged-case runs. A subsequent mixed relation ownership change has targeted test coverage but was not timed in the complete suite. |
| Delivered relation repair | `2d92d50820f1` | Avoids owned local text alongside related FTS for eligible Json/JsonLines source records; targeted test and workspace suite passed. |

Repair commits: `c1f910b16af5` (root-field semantics), `7b0c4d45fa80` (projection and subtree work), `a0c3da70bc9b` (combined source/value and relation target ownership), `c74105854736` (decoder and record scratch), `ed7adce9ceae` (formatting), `f20fe5c1c7a5` (container-only subtree skip), and `49ac3f503ac6` (avoid irrelevant checks in unscoped FTS). Each was pushed to `integration/borrowed-segment-verification`.

| Requirement | Production route | Verification |
| --- | --- | --- |
| A: borrow existing values | `reference.rs::eval_borrowed_value_fts` and `eval_borrowed_value_fts_field`; ordinary, paginated, streaming, and relation target calls in `run.rs` and `relation_query.rs` | Pointer identity, copied-byte counter, relation-visitor counter, full query suite. |
| B: borrow source bytes | `json_scalar_text.rs::scan_borrowed_segments_worker`; `run.rs` ordinary, paginated, streaming, and search verification calls | Root/duplicate differential tests, FILTER-only node counter, focused and full query runs. |
| B with required values | `json_scalar_text.rs::extract_query_record_borrowed_fts_worker`, reached through `record_preparation.rs::prepare_decoded_record_with_borrowed_fts` | Decoded-scalar residual/output test, combined copied-byte counter, high-hit logical output measurements. |
| C: reusable transformation storage | Shared quoted-token decoder in `json_scalar_text.rs`; `reference.rs::FoldRecordGuard` around record scans and evaluator calls | Decoder strategy comparison, pointer/capacity and alternating-input tests, oversized record and error-path tests. |

The source scanner and selected-field matcher share `fts_field_selects_path()`. An empty selected path covers nested scalar segments, while selected duplicate fields use the last visible member. The combined parser consumes each string synchronously with required `Value` construction and does not produce matching-only `TextSegment` strings. Generic and merged Json/JsonLines relation target traversal now sets `collect_text: false` and visits the converted target value by reference.

## Measurement setup and source data

Runs used Rust/Cargo 1.99.1 release builds on an AMD Ryzen 7 5700G (16 logical CPUs), Fedora Linux 7.2.13, and sibling worktrees on btrfs. `/tmp` is tmpfs, so the corpus, build targets, and saved measurements stayed on btrfs. Query runs were serial by revision, with four query workers, warm OS file cache, and `--verified-cache-bytes 0`. The 194-query Musenalm suite ran under ASCII and Unicode alphabets with verified-result cache off and warm. Focused synthetic runs used seven timed repetitions after warm-up; allocation counting was a separate untimed pass. Exact commands and arguments are in `focused-commands.json`, `final-focused-commands.json`, `suite-commands.json`, `recheck-commands.json`, and each suite `run-manifest.json`.

The historical affected list was derived from `../base-suite/query-results.csv` versus `../final-suite/query-results.csv` and the same-computer `../rerun-2026-11-11/{base-suite-off,final-suite-off}/query-results.csv`: 31 original case/profile rows and 81 rerun rows exceed a 5% slowdown above 1 ms, comprising 64 distinct IDs. The first current full-suite comparison added 41 IDs and the final one four more, using a >5% and >0.5 ms difference (or >1.5 ratio). `recheck-final-case-origins.csv` records which comparison flagged each ID; `recheck-final-case-list.txt` contains all 109 IDs passed to the final alternating seven-repetition recheck. `full-suite-comparison.csv`, `final-focused-comparison.csv`, and the flagged recheck comparison give per-case timings, dispersion, allocation counts where measured, and parity checks without replacing the raw results. The first 105-ID recheck remains saved for audit but used the earlier `f20fe5c1` source.

## Semantic and ownership coverage

Root-scoped literal `needle` is checked against the established evaluator for top-level and nested objects, root arrays, scalar roots, duplicate replacement by a nonmatch/null/empty container, numeric scalars, and mixed root/local selections. The integration test runs Json, JsonLines, and JsonDecodedScalars with evidence on/off and locator, bytes, and logical output. The decoded-scalar combined test checks source numeric spellings, complex residual FILTER, logical output, and the production combined call counter. Relation tests check the production borrowed target counter. Existing workspace coverage includes regex, fallback readers, streaming, cancellation, output, and explicit index update behavior.

Owned query paths retained for concrete semantic reasons:

- JsonDecodedScalars relation targets and other source-derived corpora whose searchable lexical tokens differ from the converted logical value still need owned source segments in the general relation evaluator. This includes original numeric spellings and overwritten scalar occurrences.
- Extension readers and opaque registered binding operations retain their existing owned `ExtractedRecord` or `map_text` contract; the extension may produce transformed text that cannot be borrowed from a JSON source slice or converted value.
- Explicit array query-path selectors and complex source FILTER expressions retain the established evaluator because the direct scanner only handles a prepared literal FTS and at most one compatible contains term. Regex and opaque FTS fall back to their existing prepared matchers/evaluator where direct source scanning is ineligible.
- Index construction and public extraction APIs still return owned `Value`, `TextSegment`, or `ExtractedRecord` by contract. Raw-fallback and validating unindexed inputs keep their existing reader policy. Required logical output retains its value allocation, while the combined decoded-scalar route avoids a second matching-only string allocation.

The 256 KiB limit applies separately to retained decoder, provisional-event, and folding scratch capacity, not accepted record length. The active path retains only its depth's key names and releases them on clear; evidence sets and required `Value`/output buffers are separate allocations. Oversized fold scratch is reused between segments within one record and trimmed by the record guard on success or error. Query bindings remain immutable and scan scratch is worker-local.

## Complete query suite

The final repaired binary at `49ac3f50` ran the same 194 extended queries in ASCII/Unicode and off/warm verified-cache profiles as the two controls. All 776 case/profile rows had the same status, total and returned matches, candidates, file opens, bytes read, record parses, exact postings, scanned records, residual evaluations, and verified-result hits across the three revisions. Each run manifest reports zero failures. Median of per-query repaired/control timing ratios (each query timed three times):

| Profile | Against predecessor | Against reviewed |
| --- | ---: | ---: |
| ASCII, cache off | 0.972 | 0.990 |
| Unicode, cache off | 0.925 | 1.012 |
| ASCII, cache warm | 1.048 | 1.003 |
| Unicode, cache warm | 0.948 | 0.907 |

These aggregate ratios describe the workload distribution, not each query. `full-suite-comparison.csv` lists every case with its min/median/max and exact work counters. Build timings for the two full-suite runs per revision (ASCII/Unicode medians, milliseconds) were predecessor 3073/2606, reviewed 2592/2571, repaired 2584/2615. The first predecessor ASCII build took 3548 ms; its second took 2598 ms. No repeated build slowdown follows from the shared parser change.

## Final focused runs and allocation accounting

`final-focused-comparison.csv` covers five fixed corpora, eight query shapes each, two alternating predecessor/reviewed/repaired rounds, and seven timed repetitions per run. All 80 fixture/case/round comparisons match membership digest, match count, candidate count, file opens, bytes read, and record parses. The allocation pass was outside timed samples; its gross counts include source buffers, required logical values, output, evidence, and duplicate bookkeeping. Representative medians are milliseconds for round 1 / round 2:

| Fixture and query | Predecessor | Reviewed | Repaired | Gross allocations, predecessor to repaired |
| --- | ---: | ---: | ---: | ---: |
| Plain required-value identity | 2.94 / 4.64 | 3.05 / 3.47 | 3.04 / 3.21 | 54,652 to 38,272 |
| Duplicate-heavy source identity | 10.97 / 11.64 | 2.96 / 3.30 | 3.10 / 2.97 | 956,885 to 11,561 |
| Duplicate-heavy FILTER-only | 1.57 / 1.70 | 3.00 / 2.17 | 1.47 / 1.48 | 11,912 to 7,816 |
| Duplicate-heavy combined FTS/FILTER | 11.74 / 10.72 | 3.87 / 2.91 | 2.80 / 2.86 | 957,077 to 11,753 |
| Field-duplicate scoped FTS | 6.25 / 5.19 | 1.05 / 1.07 | 1.09 / 1.29 | 287,225 to 5,417 |
| Long escaped combined FTS/FILTER | 13.90 / 13.86 | 13.95 / 14.04 | 13.75 / 15.29 | 45,946 to 15,211 |

No focused case had both a >5% and >0.5 ms repaired slowdown against either control in both alternating rounds. The long escaped combined query was 1% faster in round 1 and 10% slower in round 2 against the predecessor, with overlapping min/max ranges; the scoped long escaped query also switched direction. The duplicate-heavy source routes retain the large allocation and timing gains. Gross allocation counts are not claimed to be matching-only allocation counts: required `Value` construction is charged in full. At the changed boundaries, pointer identity verifies existing-value and plain-source borrowing, `owned_collector_copied_bytes` is unchanged for A, and `owned_segment_text_bytes` is unchanged for direct and combined B. The production combined route counter and production relation-target visitor counter are exercised by integration tests. Decoder and fold scratch capacity are checked after warm-up and after an oversized record; separate evidence and output allocations remain visible in gross counts.

The four strongest earlier Musenalm outliers were also run for 21 timed repetitions in two reverse-order rounds (`isolate-comparison.csv`). Q006 repaired medians were 639/643 ms versus reviewed 648/619 ms; Q145 was 227/230 versus 236/218 ms. Q072 was 7.24/7.33 versus reviewed 6.96/7.20 ms, an absolute difference below 0.5 ms. Q186 regex was 5.00/5.03 versus reviewed 5.15/5.33 ms. These readings contradict the apparent repeatable slowdowns in the first short recheck. Q177 has zero source reads in all revisions; its uncached median varied from 17 to 27 ms in the final full-suite control/final runs, while its warm-cache profile was about 0.01–0.02 ms. It cannot be attributed to escape decoding. The final full-suite Q191 Unicode and Q192 ASCII uncached medians were 636 versus predecessor 629 ms and 442 versus predecessor 436 ms, respectively; their source bytes and candidates were identical.

## Flagged-case recheck and remaining limits

The 109 flagged IDs produced 218 ASCII/Unicode case/profile rows in each of six seven-repetition runs, ordered predecessor/reviewed/repaired and then repaired/reviewed/predecessor. `flagged-final-recheck-comparison.csv` includes both rounds' medians, minimum, quartiles, maximum, absolute differences, and ratios for every row. Result and source-work parity held for all 218 rows. Against the predecessor, 159 rows were below the material threshold in both rounds, 43 crossed it in one round, 12 had repeated median differences but overlapping interquartile ranges, and four had separated interquartile ranges in both rounds. Against the reviewed implementation the counts were 139, 66, 12, and one respectively. The threshold was >5% and >0.5 ms. These categories are measurement dispositions, not a claim of causation.

The five separated rows were Q056 ASCII, Q069 ASCII and Unicode, Q160 ASCII against predecessor, and Q086 Unicode against reviewed. They are FILTER-only queries with identical candidates, source reads, parse counts, and results across revisions. They do not invoke source FTS or escape decoding. The limited runs do not isolate their cause; they remain unresolved performance findings. The later mixed local/relation FTS ownership change is covered by a production route test with locator and logical output and evidence on/off, but the complete suite was measured before that change. The user directed an immediate commit, push, and master merge at this point. This report does not mark the repair task's performance acceptance criteria complete.

Release-only in-memory probes for the actual existing-value visitor, direct source scanner, combined value/source consumer, and decoder strategies were added as ignored tests but were not run before the requested merge. Their numeric timings remain outstanding. The final workspace test retry passed after one known timing-sensitive desktop scheduler failure and a passing isolated retry; no Clippy rerun followed the mixed relation change.
