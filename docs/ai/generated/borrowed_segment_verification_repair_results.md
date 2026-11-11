# Borrowed-segment verification repair: final results

Task: `borrowed-segment-verification` (reopened repair). Completed on 11 November 2026. The prior repair commits through `2d92d50820f1` were merged into `master` at `95f1736d448a`. This resumption fast-forwarded the existing `integration/borrowed-segment-verification` worktree to current master `d8e3b22f850811eb58f26716c2db819016612e96`, then added `a1bb0990e86a8ed7a8c35c3e4a1610b5077c3bc3` to remove a repeatable FILTER-only slowdown. The predecessor `5166876c7228`, reviewed implementation `47931f0cdfef`, and earlier repaired implementation `49ac3f503ac6` remain separate comparison points in the [earlier repair measurements](../../../bench/results/borrowed-segment-verification/repair-2026-11-11/README.md).

## Production coverage and retained owned paths

| Requirement | Production route | Evidence and disposition |
| --- | --- | --- |
| A: borrow existing values | `reference.rs::eval_borrowed_value_fts` and `eval_borrowed_value_fts_field`, called from ordinary, paginated, streaming, and relation target evaluation | Required Json/JsonLines `Value` strings and keys are visited by reference. Pointer and copied-byte tests cover the producer/consumer boundary. The production visitor release probe measured 28 ns for a two-string record and 6,003 ns for a 513-string record. |
| B: borrow source bytes | `json_scalar_text.rs::scan_borrowed_segments_worker` for eligible direct source queries; `extract_query_record_borrowed_fts_worker` through `record_preparation.rs` when decoded-scalar output/residuals require a `Value` | Plain text and scalar lexemes borrow source slices; escaped strings use reusable decoding storage. Root-field and duplicate visibility tests, combined-route counters, full query suite, and direct/combined release probes passed. The latest FILTER-only fast path skips irrelevant scalar siblings without preparing their paths or duplicate ranges. |
| C: reuse transformation storage | `json_scalar_text.rs::with_next_quoted_text_strategy`, `BorrowedScanScratch`, and `reference.rs::FoldRecordGuard` | Decoder, provisional-event, and folding buffers have separate 256 KiB retained-capacity bounds. Oversized fold storage is reused within a record, then trimmed at its boundary, including error paths. Alternating-input and pointer/capacity tests passed. |

Owned routes remain where their semantics or API require ownership: JsonDecodedScalars relation targets can search lexical tokens that differ from converted values (including overwritten duplicates and numeric spellings); extension readers and opaque bindings retain their owned extraction contracts; explicit array selectors, complex source FILTER expressions, and regex use their established evaluator/prepared matcher fallbacks; index construction and public extraction return owned values by contract. Required logical output still allocates its `Value`, while the combined decoded-scalar consumer avoids an additional matching-only string. No search-time freshness or index mutation behavior was added.

The release-only probes at `a1bb0990` measured the actual production consumers, not the public owned extraction control:

| In-memory shape | Median per record/token |
| --- | ---: |
| Direct plain source, 8,209 bytes | 592 ns |
| Combined plain source and required value, 8,209 bytes | 1,363 ns |
| Direct escaped source, 6,161 bytes | 18,392 ns |
| Combined escaped source and required value, 6,161 bytes | 16,516 ns |
| Direct many-scalars source, 3,091 bytes | 11,862 ns |
| Combined many-scalars source and required value, 3,091 bytes | 50,157 ns |
| Existing-value visitor, 2 / 513 strings | 28 / 6,003 ns |

The decoder probe compared reusable strategies with predecessor-style owned serde decoding on identical tokens. For dense simple escapes, adaptive-8 took 17,627 ns/token versus 17,469 for predecessor serde-owned; for Unicode escapes, 10,172 versus 9,956; for surrogate pairs, 6,665 versus 6,328. Plain ASCII was 362 versus 1,080, and sparse simple escapes 470 versus 1,751. The small escape-heavy timing cost is the cost of safe reusable decoding; the earlier two-round focused measurements did not find a material repeated end-to-end slowdown, and gross allocations fell sharply. For example, duplicate-heavy source identity fell from 956,885 to 11,561 allocations and field-duplicate scoped FTS from 287,225 to 5,417 in the prior focused comparison at `49ac3f50`. These gross counts include required source/output/evidence work; the boundary counters and pointer tests establish the specific matching-copy reductions. Raw current probe logs are `borrowed-consumers.log`, `borrowed-values.log`, and `decoder-strategies.log` under the [overnight results directory](../../../bench/results/borrowed-segment-verification/overnight-2026-11-11/).

## Q069 regression and repair

The 11 November review had five FILTER-only timing outliers despite identical candidates and source work. Q069 (`pseudonyms ~ ""` over 4,010 agent records) was repeatably slower in fresh alternating seven-sample controls: predecessor medians were 14.867/14.871 ms ASCII and 14.426/14.699 ms Unicode, while the current pre-fix branch was 18.526/17.211 ms ASCII and 17.380/16.977 ms Unicode. The reviewed borrowed implementation was similarly slower, so the issue was inherited from its direct scanner.

The cause was generic object traversal on a FILTER-only scan: every unrelated scalar sibling entered `scan_borrowed_value`, performed path and applicability checks, and the selected key used an owned duplicate-key map entry per record. Commit `a1bb0990` makes the one selected child explicit. Unrelated children go directly through the existing trusted skipper, while one previous event range preserves duplicate replacement without allocating a key/tree node. This keeps the same source read and parse policy. A work-counter test with 1,000 irrelevant scalar siblings and a large nested subtree now visits fewer than 10 scanner nodes; nested duplicate replacement, escaped selected text, empty substring, missing/null, and multivalue semantics are tested.

| Matched Q069 run, cache off, 7 samples | ASCII median, ms | Unicode median, ms |
| --- | ---: | ---: |
| Current master before fix, first round | 16.785 | 17.563 |
| Repaired `a1bb0990`, first round | **13.527** | **14.340** |
| Repaired `a1bb0990`, reverse round | **14.036** | **14.135** |
| Current master before fix, reverse round | 16.418 | 16.744 |

Each row returned 4,010 matches and visited 4,010 candidates, opening/parsing 4,010 files and reading 29,483,280 bytes. The full-suite medians were 16.554 → 14.332 ms ASCII and 18.511 → 15.152 ms Unicode with the same counters. The first narrower duplicate-map-only experiment had no reliable timing gain; the skip of irrelevant scalar recursion is the measured improvement.

## Complete suite and regression disposition

The unchanged 194-query suite ran three repetitions in ASCII/Unicode, cache off/warm, four query workers, with existing count, evidence, return modes, limits, and sort settings. The exact-revision primary comparison is `master-suite` at `d8e3b22` versus `final-suite-matched` at `a1bb0990`. Both used the same absolute corpus root. Both manifests report corpus fingerprint `f00b5967b2c54196fb5e6d185b8eee919c9b4f490aa483078943292036bc3e65` and schema fingerprint `3218898bb28ff68620caf0347f3d69e1440c0f808fdd68742367b0e2453c7e81`; their `queries.json` files have the same SHA-256 `5bcf5883d02e2d6bca75aac51bd4313ca65336fd281d958cceb3850de0f76d2a`. The manifest's `workload_fingerprint` differs because the harness hashes the manifest path as well as bytes and the binaries were built in different worktrees. The earlier `final-suite` used a relative root and is retained only as preliminary screening data.

**All 776 profile rows passed.** Baseline and final agree on every row's total matches, returned count, candidates, source opens, bytes, parses, scans, and residual-evaluation count. Q115–118 and Q120 retain zero query source opens, bytes, parses, and scans with cache off. Estimated index and snapshot sizes are identical between these revisions: 69,662,968 / 129,139,458 bytes ASCII and 72,571,368 / 131,650,034 bytes Unicode. One build per alphabet measured 2.552 → 2.621 seconds ASCII and 2.574 → 2.643 seconds Unicode; no build code changed, and the roughly 2.7% difference is below the 5% investigation threshold. Peak RSS was 568,807,424 → 591,265,792 bytes ASCII and 795,930,624 → 824,786,944 Unicode, also below 5%.

The three-sample full suite displayed 29 ASCII-off and 32 Unicode-off rows above both 5% and 0.5 ms, with no warm-profile row above that threshold in the matched comparison. To investigate rather than infer from aggregate medians, 72 distinct flagged IDs from both full-suite screenings were remeasured in two alternating seven-sample rounds with both cache-off alphabets. All 144 result/work rows in each run matched. Four ASCII rows crossed the threshold in both short rounds: Q022, Q110, Q049, and Q158; no Unicode row did. Longer 100-sample reverse-order isolates showed their medians converging:

| Case, ASCII off | Master round 1 | Final round 1 | Final round 2 | Master round 2 | Changed scanner path? |
| --- | ---: | ---: | ---: | ---: | --- |
| Q022, unscoped FTS | 10.190 | 10.290 | 10.182 | 10.287 | No FILTER-only path |
| Q110, explicit null FILTER | 16.633 | 16.418 | 16.649 | 16.438 | No, direct scanner ineligible |
| Q049, Boolean posting | 0.317 | 0.311 | 0.316 | 0.318 | No source scan |
| Q158, field-to-field FILTER | 16.734 | 17.337 | 16.720 | 17.430 | No, direct scanner ineligible |

Q190 Unicode warm, another preliminary outlier, converged in a 21-sample reverse-order isolate at 81.306/81.289 ms master versus 80.651/80.633 ms final. Among the five original findings, Q056 and Q086 use exact index-only paths and their short medians moved in both directions; Q160 uses the existing explicit-selector residual path, with unchanged 70,390 source opens/parses and mixed timing direction; Q069 had the direct-scanner regression and is fixed above. The brief full-suite and seven-sample outliers are run-condition variance in paths unaffected by the changed FILTER-only branch, as supported by unchanged plans/work counters and the longer paired distributions. No repeatable material regression remains in these measurements. The result does not assert identical latency on other hardware.

## Checks and reproduction

`cargo test --workspace --locked` passed (including 501 core unit tests and 158 desktop tests); the focused core unit run passed before the full suite. `cargo clippy --workspace --all-targets --locked` passed with existing warnings. `rustfmt --edition 2024 --check` for the two changed Rust files and `git diff --check` passed. Logs are committed with the raw results.

Build each revision with `cargo build --release --locked --example bench_musenalm`. From the repository root, run the same binary with `--root /home/simon/source/objekte-integration-borrowed-segment-verification/examples/musenalm --extended --profiles ascii-off,unicode-off,ascii-warm,unicode-warm --repetitions 3 --query-workers 4 --verified-cache-bytes 67108864 --result-dir <directory>` for the complete suite. The paired controls used `--cases Q056,Q069,Q086,Q160` and `--profiles ascii-off,unicode-off --repetitions 7 --verified-cache-bytes 0`; the 72-ID rechecks and 21/100-sample isolates used the case lists and commands recorded in their manifests. The current in-memory probes used `cargo test --release --locked -p objekte-search-core --lib <probe-name> -- --ignored --nocapture --test-threads=1`, with probe names `borrowed_consumer_microbench`, `borrowed_value_consumer_microbench`, and `decoder_strategy_microbench`. Timing and allocation runs were separate. The raw result directories contain individual measurements, plans, index results, CSV summaries, and full command manifests.
