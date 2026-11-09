# Index lifecycle repair validation

Task: `index-lifecycle-repair`  
Baseline: `63f354b2b3d0bb7749f8af5187919c600767a8ea`  
Integration branch: `integration/index-lifecycle-repair`

This report records measurements from the combined integration branch. Raw outputs and selected native screenshots are in [`raw/`](raw/). Timings are engineering measurements on a warm local filesystem cache, not universal latency guarantees.

## Environment and comparability

- Fedora kernel `7.2.15-200.fc44.x86_64`, Intel Core i9-13900K, 32 logical CPUs; Rust/Cargo 1.99.1; release profile for benchmarks, debug profile for tests and native GUI.
- Musenalm: 148,486 JSON files and records, 603 MiB source tree, seven configured collections. Each mutation run used an independent disposable copy with `OBJEKTE_BENCH_MUTATE_COPY=1`. The extended workload file SHA-256 is `edd9ace84c1df94692e4c626d942d334b4fae0731f258477267be97d3ed967b2`. The static benchmark manifest records corpus fingerprint `9f93a4ba7fb9249c9c4eb04e03ed911435a6b0493f15b087a5447add32456aa9`, schema fingerprint `a7cff399f67082c6153b1245b24a8a392a50a5dd69563762047b5c15ec944433`, and resolved 194-case workload fingerprint `01d846ea70d4fb3e4c4a98c28f4a48400b9505f0d4e21bcaa4251dbcff52e7ec`.
- Historical reference: [`index-lifecycle-performance-validation.md`](../index-lifecycle-performance-validation.md), built on this machine with release Rust 1.99.1 and the same corpus. Its 148,486-file three-run medians were 1,317.728 ms fresh build, 1,278.918 ms original-base verified update, 0.329 ms ordinary appended update, and 1,122.551 ms compaction. The historical static query matrix has five repetitions for each of 194 cases in ASCII and Unicode profiles.
- Current storage stress: `cargo build --locked --release -p objekte-search-core --example bench_lifecycle_mutations`; then `OBJEKTE_BENCH_MUTATE_COPY=1 target/release/examples/bench_lifecycle_mutations stress <fresh-copy>` under `/usr/bin/time -v`. Three independent runs, raw `admission-final-{1,2,3}.{tsv,stderr}`. The benchmark uses a concurrent query probe during maintenance.

## Implementation and focused verification

| Requirement | Integrated behavior | Focused regression evidence |
| --- | --- | --- |
| R1 cache | `IndexView` shares a bounded, instance/revision-scoped verified cache across publications and callers; ordered membership can serve later pages. CLI, TUI, and desktop route through the view. | `desktop_page_reuses_verified_order_and_keeps_pinned_revision`, `measure_desktop_lifecycle_query_cache`, selected Musenalm cache profiles below. |
| R2 stream/export | Stored-order lifecycle cursors deliver page rows before optional counting; all-results delivery uses bounded batches and stops on cancellation or sink failure. | `ordered_rows_arrive_before_count_finishes_and_reconstruct_page`, `stored_order_cursor_merges_collection_ties_before_delivery`, `measure_large_lifecycle_all_export_streams_before_expanded_locator_budget`. |
| R3 scale | Batched reconciliation deduplicates overlapping physical observations and delays absence proof; source-free compaction processes bounded shards without a total-slot ceiling. | `reconciles_300000_files_and_updates_a_shared_file`, `large_source_free_compaction_cancels_and_retries_a_concurrent_commit`, reduced-budget overlap and tombstone tests. |
| R4 policy | Desktop replacement transfers the live interval/enabled policy under operation generations and a policy gate; UI reflects the replacement scheduler. | `loaded_index_receives_check_policy_before_old_scheduler_retires`, native periodic update after cache replacement and Cancel close. |
| R5 close | Quiescence blocks publication, stops/drains mutation producers and automatic writer, then saves the final accepted revision; retry and cancel restore a usable session. | `concurrent_publication_is_either_saved_or_rejected_by_quiescence`, `pending_automatic_save_enable_cannot_write_after_manual_close`, backend final-save tests, native save-failure/Retry/Cancel/Close without saving. |
| R6 local verified | Fresh and restored bases use bounded 2,048-record shards; eligible one-record verified updates physically replace a slot while pinned readers keep old state. | `restored_shared_partition_verified_update_preserves_its_record_slot`, `slot_preserving_replacement_keeps_local_address_and_rebuilds_all_routes`, real Musenalm three-run update timings below. |
| R7 move | Compatible path-independent moves verify destination digest, retain content postings and record slots, and route reads through the pinned catalog; unsuitable cases use replacement. | `compatible_move_keeps_index_contributions_and_advances_file_version` and cache/compaction/path tests; three-run large-file benchmark below. |

### API and snapshot compatibility

The lifecycle API exposes `Index::configure_verified_cache` and existing `IndexView` search, count, and stream entry points for the shared verified cache. `Index::quiesce` establishes the closing publication barrier; `resume_after_quiesce` restores a cancelled close. The desktop backend's `shutdown_with_final_save` uses the barrier before flushing the selected target. `move_file` retains its public index-only contract: the caller has already moved the source file.

Static snapshots remain version 17; partitioned lifecycle snapshots use explicit version 18. A lifecycle load validates the version, configuration hash, file catalog, and private segment sections. An incompatible older snapshot is rejected by explicit load and follows the existing configured rebuild policy on convenience open. A rejected load is not reported as restored. The verified query cache remains memory-only, bounded across pinned revisions, and is not serialized into either snapshot format.

## Correctness and scale

`cargo test --locked --workspace --all-targets` passed at implementation `66a0260e75`: 465 core tests (11 ignored), 116 desktop tests (1 ignored), 77 CLI tests, 32 CLI process tests (1 ignored), 49 TUI tests, and all remaining targets. Raw output: `raw/workspace-tests-final.log`. The ignored scale and benchmark tests were run separately as described below.

The ignored scale test `reconciles_300000_files_and_updates_a_shared_file` ran against 300,000 physical files and 450,000 logical records, including two overlapping collections. The build took 8,589 ms; the unchanged pass took 4,671 ms and a shared-file update pass took 4,623 ms. Maximum resident memory was 737,396 KiB. It completed with no issues. Command: `timeout 600s cargo test --locked -p objekte-search-core --lib reconciles_300000_files_and_updates_a_shared_file -- --ignored --nocapture`. Raw: `raw/reconciliation-300000.log`.

A final rerun added direct assertions on content work: the unchanged 300,000-file pass indexed zero source bytes; the one shared file with two collection memberships indexed exactly 16 changed bytes once. It again examined 300,000 physical files, retained 450,000 logical records, and passed. Raw: `raw/reconciliation-300000-final.log`. Its whole-process maximum RSS includes a five-second test compilation and is not a comparable index-only peak; the earlier precompiled run supplies the index scale memory figure above.

The generated source-free compaction test passed above the former 262,144-slot limit and again at 1,000,001 active records. It retained an older reader, removed source files, cancelled one attempt, and forced a concurrent commit/retry. Results:

| Active records | Build | Compact | Publish attempts | Maximum RSS |
| ---: | ---: | ---: | ---: | ---: |
| 262,144 active + 1 retired | 755 ms | 227 ms | 2 | 2,150,516 KiB |
| 1,000,001 | 7,620 ms | 883 ms | 2 | 1,016,684 KiB |

Raw outputs are `raw/compact-tombstone-ceiling-v2.{stdout,stderr}` and `raw/compact-1000001.{stdout,stderr}`. A small-budget test also compacts nine allocated slots with seven retired and source files absent. The different RSS peaks reflect different process conditions and should not be interpreted as a decreasing memory curve.

## Real-corpus storage results

Three independent Musenalm stress runs on the combined direct-build and sparse-admission 2,048-record shard implementation all passed. Medians below are wall times in milliseconds, with the observed range in parentheses. Raw final runs: `raw/admission-final-{1,2,3}.{tsv,stderr}`.

| Phase | Current median (range) | Historical reference | Result |
| --- | ---: | ---: | --- |
| Fresh build | 1,321.780 (1,318.993–1,324.729) | 1,317.728 | within 0.3% |
| Ordinary add | 0.735 (0.626–0.836) | 0.302 from a separate mutation mode | sparse and sub-ms |
| Ordinary append update | 0.561 (0.526–0.711) | 0.329 | 0.232 ms slower, still sub-ms |
| Original-base verified update | 45.585 (44.925–45.840) | 1,278.918 | 28× faster; `VerifiedSlotPreserving` |
| Untouched original-base verified update | 40.416 (39.040–40.747) | 1,278.918 | 32× faster; `VerifiedSlotPreserving` |
| Source-free compaction | 1,628.784 (1,457.796–1,641.510) | 1,122.551 | 45% slower |
| Rebuild while querying | 1,414.169 (1,379.157–1,514.475) | 1,230.018 | 15% slower |
| Cache flush | 668.274 (655.786–673.510) | 307.026 | larger partitioned snapshot |
| Cache decode to searchable | 1,604.095 (1,603.750–1,618.686) | not directly comparable | different measured boundary |
| No-change scan | 328.066 (326.691–328.575) | 910.805 | faster |

The verified update no longer rewrites the collection. Its measured affected-shard work was about 15–16 ms for old-shard compaction, 3–4 ms for consolidation, and 12–13 ms for dense remapping, with a 2,048-record shard. The operation still misses the single-digit-millisecond engineering target. All three runs reported `VerifiedSlotPreserving` with no fallback. Steady query probe p50 was about 0.68–0.70 ms, with zero errors; process peak RSS was about 2 GiB. These probe numbers cover a simple locator page, not the full query matrix.

The earlier 3.425-second fresh-build regression was caused largely by building a static collection and projecting its 133,948-record dominant collection into 66 shards. The direct built-in JSON path admits extraction batches into bounded shards and removes that projection. It preserves a fallback for JSONL, custom readers, and oversized source files. Larger shard trials saved too little build time while substantially increasing local verified-update cost, so the 2,048-record budget remains. A second regression came from counting the 66 full base shards against a 64-segment sparse-update budget: each append unnecessarily consolidated dense state. Reserving 64 additional sparse slots for small deltas reduced the update median from 4.969 to 0.561 ms without a collection-sized copy. Earlier direct and projection trials remain in `raw/musenalm-direct-final-{1,2,3}.tsv`, `raw/musenalm-project-phases.tsv`, and `raw/musenalm-partition-{4096,8192}.tsv`.

## Compatible move comparison

The ignored release test `large_compatible_move_skips_extraction_and_posting_rewrites` indexed 50,000 JSONL records in one file, then compared a same-content rename against a content-changed move on three independent fixtures. The compatible route hashed 1,977,780 destination bytes to prove content equality; it did not re-extract, parse new records, create shards, or rewrite posting keys. The fallback parsed 50,000 records and wrote 26 new shards with 100,000 posting keys. `extraction_calls` is inferred from the returned single-writer strategy; parsed-record, shard, and posting counts are inspected directly.

| Route | Three wall times (ms) | Median (ms) | New extraction calls | Parsed records | New shards | Posting keys rewritten |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Compatible, digest-proven | 1.156 / 1.127 / 1.132 | 1.132 | 0 | 0 | 0 | 0 |
| Changed-content fallback | 98.108 / 93.397 / 94.982 | 94.982 | 1 | 50,000 | 26 | 100,000 |

The compatible route is about 84× faster in this fixture, but its 1.132 ms still includes file-size I/O for the digest; it is not a constant-time rename. Command: `cargo test --locked --release -p objekte-search-core --lib large_compatible_move_skips_extraction_and_posting_rewrites -- --ignored --nocapture` repeated three times. Raw: `raw/large-move-{1,2,3}.{stdout,stderr}`.

## Native desktop check

The native GPUI application ran in a verified Sway/WayVNC session at 1920×1080, scale 1, 60 Hz. A disposable configured 130-file collection was opened in the desktop. It displayed all 130 records after pagination and returned the matching field for `Item 129`. The application saved and reloaded an index snapshot. Periodic checking remained enabled across reload; after externally changing record 129, the UI marked results outdated, and Refresh displayed `Item 129 Periodic`. A record tab was opened, and the index maintenance panel exposed compaction and automatic saving. Selected screenshots: `raw/configured-opened.png`, `raw/paginated-bottom.png`, `raw/searched.png`, `raw/periodic-refreshed2.png`, `raw/record-tab.png`, `raw/auto-save-on.png`.

The native save-failure path was exercised by removing write permission from the disposable fixture's `.objekte` directory, changing an indexed file, and asking Sway to close the application. The window remained open and showed a responsive `Permission denied` dialog with Retry, Cancel close, and Close without saving index. Cancel close returned to the search view; the Index panel showed periodic checking enabled, and a subsequent external edit triggered another outdated-results notice. Restoring write permission and choosing Retry closed the app and advanced the selected snapshot. Repeating the failure and choosing Close without saving closed the app while the snapshot timestamp and size stayed unchanged (`1794246624`, 62,313 bytes). The fixture permission was restored. Screenshots: `raw/close-save-failed.png`, `raw/cancel-index.png`, `raw/cancel-periodic-detected.png`.

The native fixture did not exercise a long in-flight rebuild during close. That race is covered by deterministic core/controller tests; its native visual behavior remains a validation limit.

With the `items/001` record tab open, native compaction kept the tab and marked its previously loaded content as possibly changed. Refresh retained the record and displayed its fields. After changing the disposable `001.json` source and running **Check for changes**, the tab again kept its previous content with a stale label; Refresh displayed `Item 001 Replaced`. Screenshots: `raw/native-tab-open-before-compact.png`, `raw/native-tab-survived-compact.png`, and `raw/native-tab-refreshed-replacement.png`. The desktop exposes source checking and compaction but has no direct compatible-move command; the direct move API, pinned tab-equivalent views, source routing, and persistence are covered by core tests rather than a native menu action.

## Query validation

The ignored desktop-backend timing test `measure_desktop_lifecycle_query_cache` ran seven successive counted, sorted `QuerySpec` pages through `query_page_on_view` on a disposable 2,000-file fixture. The first execution took 48.632 ms, missed the cache, and scanned 2,000 records. The next six took 0.364–0.431 ms each, hit the verified cache, and scanned zero records. Command: `cargo test --locked -p objekte-desktop --lib measure_desktop_lifecycle_query_cache -- --ignored --nocapture`. Raw: `raw/desktop-cache.log`. This measures the actual desktop backend, not a standalone static repository search.

Five representative Musenalm queries were run five times with the lifecycle cache disabled, then once cold and five times warm with a 64 MiB shared verified cache, for both ASCII and Unicode on the final code and original source tree. All 50 warm samples reported a cache hit and zero scanned records; exact counts matched cache-off execution. The table gives five-run medians in milliseconds for disabled and warm runs; cold is the single first cached execution. Cache-off medians are taken from `raw/final-matrix-66a0260e.jsonl`; cache-on raw is `raw/lifecycle-cache-final-on.jsonl`.

| Case | ASCII off | ASCII cold | ASCII warm | Unicode off | Unicode cold | Unicode warm |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q001 broad FTS | 21.610 | 142.790 | 0.013 | 21.002 | 144.001 | 0.012 |
| Q057 exact field | 0.096 | 1.996 | 0.011 | 0.093 | 1.953 | 0.011 |
| Q166 relation term | 16.637 | 69.456 | 0.011 | 16.755 | 68.435 | 0.011 |
| Q173 relation regex FTS | 24.886 | 168.307 | 0.011 | 24.752 | 168.783 | 0.012 |
| Q175 relation range | 38.066 | 124.603 | 0.024 | 37.051 | 124.976 | 0.023 |

The first cached execution retains complete membership and ordering for later pages, and currently uses the serial cache-building route. It is therefore slower than the bounded parallel cache-disabled route on these cases, by 1.9–143 ms absolute. This is a measured cold-cache tradeoff, not a hidden warm-cache result. Subsequent counted queries and later pages reuse the same revision-scoped entry in 0.011–0.024 ms on this corpus. The older selected-run raw `raw/lifecycle-cache-selected-{off,on}.jsonl` records the pre-optimization checkpoint and is not used for the final table.

The ignored `measure_large_lifecycle_all_export_streams_before_expanded_locator_budget` test generated 24,000 rows whose expanded locators would use 80,592,000 bytes. On the combined branch, first batch arrived in 0.244 ms, complete export took 70.133 ms, and peak buffered data was 57,260 bytes. Its fixture build took 1,142 ms. Command: `cargo test --locked -p objekte-search-core --lib measure_large_lifecycle_all_export_streams_before_expanded_locator_budget -- --ignored --nocapture`. Raw: `raw/large-export.log`.

The lifecycle query harness now accepts `LIFECYCLE_COUNT_OFF`, `LIFECYCLE_OFFSET`, `LIFECYCLE_LIMIT`, and `LIFECYCLE_RETIRE_PATH` for controlled page comparisons. Retirement changes only the in-memory index after building; it does not edit the source tree. Eight selected runs covered the Cartesian product of base/retired view, count on/off, and offset 0/100 for Q057 (exact field), Q135 (stored sort), and Q192 (computed positional sort), with five repetitions in each text profile. All runs succeeded. Q057's ASCII medians were about 0.09 ms for the first 50 rows and 0.21 ms at offset 100; count-off returned `count: null`. Base and retired views gave the same selected-query counts because the retired agent file was outside these selected result scopes. Computed-sort Q192 necessarily scanned 133,948 records; its offset-100 page initially missed the bounded parallel route and took about 1.2 seconds. The later guarded route restored a targeted 175 ms median at offset 100 with the same 70,386 exact matches. Raw: `raw/pagination-*.{jsonl,log}` and `raw/q192-offset100-parallel.jsonl`. These selected measurements are checkpoints; final query comparisons are recorded below after the last execution change.

The first full integrated matrix after the explicit-array selector fix completed 194 cases × ASCII/Unicode × five repetitions = 1,940 successful lifecycle samples. Its totals, returned rows, exact-count flags, and completeness matched the one-repetition master static run made against the same November-mtime disposable corpus. A separate five-repetition master static run used the original October-mtime source tree and completed all 1,940 samples with zero failures. Q162 filters `@file.modified = 2026-10`: it correctly returns zero on the November copy and 4,928 on the October source. After restoring the disposable fixture's 148,486 file modification times to match the original (and verifying sizes), a targeted lifecycle Q162 run returned 4,928 matches and 50 rows in both ASCII and Unicode, with zero source opens. The complete final differential uses the original source path for both implementations. Raw: `raw/final-integrated-14fbb12fab.jsonl`, `raw/static-master-one-rep-measurements.jsonl`, `raw/static-master-five-measurements.jsonl`, and `raw/q162-aligned.jsonl`.

### Complete cache-disabled query matrix

At integration `66a0260e75`, `cargo run --locked --release -p objekte-search-core --example bench_lifecycle_musenalm -- /home/simon/source/objekte/examples/musenalm <output.jsonl>` ran all 194 cases, five times in each text profile, with 32 query workers and the verified cache disabled. All 1,940 lifecycle executions succeeded. Each case/profile matched the five-repetition static master reference for total matches, returned row count, and exact-count/completeness flags. Q162 returned 4,928 matches and 50 rows in both profiles. Raw: `raw/final-matrix-66a0260e.{jsonl,log}`, `raw/final-comparison-66a0260e.txt`, and `raw/static-master-five-{measurements.jsonl,run-manifest.json,query-results.csv,REPORT.md}`.

Medians below are medians of each case's five-run median; ratios are computed within each case before taking their median. The static baseline used the same source path, profiles, worker count, query options, and cache-disabled mode.

| Profile | Lifecycle median case | Static median case | Median per-case ratio | Cases over 2× |
| --- | ---: | ---: | ---: | ---: |
| ASCII off | 1.293 ms | 0.809 ms | 1.381× | 44/194 |
| Unicode off | 1.240 ms | 0.763 ms | 1.388× | 53/194 |

The first integrated diagnostic on the November copy had an 8.81× ASCII median case ratio and 165/194 cases over 2×. Its query options were the same, but the source mtimes differed for Q162, so its timings are diagnostic only. The complete result above follows the successive exact shard proofs, bounded parallel residual/FTS and relation verification, stored-order k-way pages, and conservative positional candidate pruning. Representative ASCII five-run medians, in milliseconds:

| Case | Early lifecycle | Final lifecycle | Static master | Main change |
| --- | ---: | ---: | ---: | --- |
| Q119 inferred exact filter | 1,149 | 11.580 | 12.610 | exact shard proof and top-page ranking |
| Q134 stored-sort first page | 85.7 | 1.079 | 4.279 | whole-shard typed extrema proof and k-way page |
| Q135 stored-sort later page | 89.7 | 1.476 | 6.372 | same proof and cursor |
| Q136 stored-sort deep page | 115.2 | 4.458 | 27.352 | same proof and cursor |
| Q160 positional array filter | 1,172 | 112.6 | 68.371 | unpositioned posting supplies a safe `may` set; 70,390 rather than 133,948 source reads |
| Q173 related regex FTS | 198 | 24.886 | 19.847 | parallel target verification |
| Q174 FTS plus related filter | 831 | 36.2 | 16.540 | bounded parallel source and target verification |
| Q181 related-field FTS | 538 | 20.798 | 15.872 | same bounded relation workers |
| Q192 computed positional sort, offset 100 | 1,204 targeted | 175 targeted | no directly identical static page run | bounded parallel local sort at the requested offset |

The remaining largest cold differences are broad source-reading queries. Q116, Q117, Q120, and Q187 open/parse 133,948 files in both implementations; lifecycle medians are 161–195 ms versus 96–128 ms static. Broad FTS Q006/Q007 opens 148,486 files in both and takes 272/321 ms versus 211/262 ms. Q160 now opens 70,390 files and takes 113 ms versus 70,390 files and 68 ms static. The lifecycle path checks pinned file identity and revision around reads and merges bounded shard workers; these are plausible contributors to the remaining per-file cost, inferred from code and matched work counters rather than isolated cycle attribution. There is no additional collection-sized source scan in these cases. Q052 remains 22 ms versus 3.8 ms with zero source opens after exact per-shard range evaluation.

Source-free relation cases Q167/Q175 remain about 42/38 ms versus 5.4/4.8 ms static. A separate 20-sample phase profile on the aligned corpus measured 1.7–3.5 ms for target proof, 31.0–36.5 ms for constructing source bitmaps, and 0.09–0.18 ms for ordered page delivery. Within source bitmap construction, 55,247 matching posting keys across shards took 5.9–8.9 ms to look up and 24.5–27.3 ms to union; 76,889 keys were visited for 95,153 exact source matches. A tested target-ID inversion saved only 1–4 ms and was discarded. The remaining 30–38 ms is measured partition posting fan-out; removing it needs a broader reverse directory or storage representation change with its own maintenance cost. Raw: `raw/direct-phase-detail.{log,jsonl}` and `raw/bitmap-inversion-diagnostic.jsonl`. These differences are visible cold-path limits of the partitioned representation; warm verified-cache queries return in microseconds as measured above.

## Final checks

At implementation `66a0260e75`, `cargo test --locked --workspace --all-targets` passed as detailed above. `cargo check --locked --workspace --all-targets` passed; it emitted the existing unused `UiState::index_mode` warning. Raw outputs: `raw/workspace-tests-final.log` and `raw/cargo-check-final.log`. `git diff --check 63f354b2b3..HEAD` passed.

`cargo fmt --all -- --check` remains repository-wide failing: the integration reports 101 files versus 107 on the unchanged `master` baseline, with **zero new failing file paths**. This comparison normalizes the two worktree roots and does not reformat unrelated code. Raw output: `raw/cargo-fmt-final.log`; the baseline log is in the original checkout's `target/lifecycle-repair-master-fmt-check.log`.

`bash scripts/gui/run_desktop.sh prepare` rebuilt and launched the final native desktop binary in the Sway/WayVNC session. `desktop_control.py status` verified the live window at 1920×1080, scale 1, 60 Hz. The screenshot `raw/native-final-smoke.png` shows the expected initial window. The task-owned app was closed after the smoke test; the prior representative collection, tab, periodic-update, and save-failure native checks above remain the substantive UI evidence.

The remaining measured limits are the 40–46 ms verified base update versus a single-digit-ms target, 45% slower source-free compaction, 15% slower rebuild, and cold query cases with partition posting fan-out or per-file verification costs. The report gives absolute times, counters, and comparable baselines. No validation result here implies those costs disappeared in warm cache use. A long in-flight rebuild during native close and a direct compatible-move menu were not visually exercised; deterministic core/controller tests cover the former, and the desktop has no direct move menu for the latter.
