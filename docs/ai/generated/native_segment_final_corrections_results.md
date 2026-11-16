# Native segment final corrections results

Task identifier: `native-segment-final-corrections`

## Revision and environment

- Actual starting revision: `7e88607d1e3ed929e4ac9693e1f56a064d8ac9b1`
- Final tested implementation revision: `170eaeca03efa4cbc591a43fc3e611dd333b6ad2`
- Branch: `integration/native-segment-construction-repair`
- Integration worktree: `/home/simon/source/objekte-integration-native-segment-construction-repair`
- Toolchain: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`
- `Cargo.lock` SHA-256: `dc29ff42cf24b2cc4de67a2297f3c4d676c9034c0b91315465d1fb1136b486f7`
- Construction benchmark binary SHA-256: `28dc7a356919ef256a483fbb7853b84561ed9ab6f472d095850dc8c59a8be8ec`
- Query benchmark binary SHA-256: `c0cbf96fd8a3c23a74c5895f2a2b574a5cc576643fd9a563519ee5726679b097`
- Musenalm corpus fingerprint: `b79783c538f9fb76b284cc1535a36d4c9864d5d4d4728e698704eaea9ef0b5fd`

Implementation commits:

| Commit | Scope |
| --- | --- |
| `5344b4f374` | F01: current-catalog target reads and related-FTS lifecycle regression |
| `dee6d5f545` | F02: collection-wide short-string coverage and dictionary-admission regressions |
| `5103218c13` | F03: complete private-worker draining and deterministic final-drain regression |
| `170eaeca03` | Warning-free duplicate-completion preservation in the F03 cleanup path |

The report commit is intentionally later than the tested implementation revision.

## Correction completion

| ID | Changed behavior and functions | Regression | Reviewed-baseline evidence | Final result |
| --- | --- | --- | --- | --- |
| F01 | `SourceAccessSession` now selects either an inventory or a borrowed current `FileCatalog`; `handle` resolves current relative paths and revisions; `verify_qualified_targets` uses the pinned catalog | `relation_semijoin::tests::related_fts_reads_current_target_revisions_across_file_lifecycle_and_compaction` | Failed with `unknown source file` after adding a target absent from the original inventory | Passes through target add, replacement with changed length, target rename, live compaction, and deletion; related FTS reads the current target revision and reports `entry.title` evidence through the optimized path with cache disabled |
| F02 | Shared `short_dictionary_covers_required_domain` enforces per-segment semantic-domain equality; aggregate retained-posting and all-domain facts drive final completeness; diagnostics and physical posting capabilities use the same result | `partial_short_values_do_not_override_failed_general_dictionary_admission`, `short_dictionary_coverage_is_independent_of_segment_order`, `complete_short_and_general_dictionaries_remain_exact_positive_controls`, `short_dictionary_coverage_respects_null_missing_and_uncovered_domains` | The mixed `512 + 1` fixture incorrectly reported `value_dictionary_complete: true`; three focused tests failed | 512 long values plus one short value no longer claim a complete dictionary; partial short candidates and residual verification remain available; all-short, admissible general, missing, null, and uncovered-domain controls pass |
| F03 | Private completion receive and process are separate; `drain_completions` retains the first worker error while consuming every submitted completion; `publish_private_segment_group`, discard, extraction failure, submission failure, cancellation, and invalidation share the drain behavior | `lifecycle_initial::tests::private_final_drain_consumes_worker_error_before_returning` | With the reviewed early-return drain restored, the build returned while the second worker remained blocked | An error first observed in final drain is retained, the second completion is consumed, outstanding accounting reaches zero, the original error is returned, and no segment, searchable membership, inference contribution, or relation route is published |

Reviewed-baseline failure excerpts are preserved in [baseline-failures.txt](native-segment-final-corrections/baseline-failures.txt).

## Validation

Final commands at `170eaeca03`:

| Command | Result |
| --- | --- |
| `cargo test --workspace --locked --no-fail-fast` | Passed: core 543 passed and 16 ignored; CLI and desktop unit/integration targets passed; all doctests passed |
| `cargo check --workspace --all-targets --locked` | Passed |
| `cargo clippy --workspace --all-targets --locked` | Passed with repository-existing advisory warnings; no warning from the changed paths |
| `cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked` | Passed |
| `git diff --check` | Passed |

The former `private_segment_window_is_bounded_for_one_and_multiple_workers` assertion incorrectly bounded `build_peak_segment_jobs` by the worker count. That counter records segment jobs that have entered but not yet finished, not worker threads. The regression now verifies that the counter ran and bounds the scheduler's actual outstanding-job window by `2 * workers`; the complete workspace suite passes.

## Construction performance

The established three-repeat procedure was used at eight build workers:

```bash
target/release/examples/bench_build_musenalm \
  --root examples/musenalm \
  --alphabet <ascii|unicode> \
  --build-workers 8
```

| Alphabet | Prior median | Final median | Change | Final peak RSS |
| --- | ---: | ---: | ---: | ---: |
| ASCII | 814.378 ms | 876.767 ms | +7.7% | 577.044 MB |
| Unicode | 899.332 ms | 887.279 ms | -1.3% | 536.764 MB |

All runs produced 148,486 records and 15 native segments. ASCII retained index signature `60fc465c…47ffd`; Unicode retained `dfe2ba47…a2cc9`. With eight configured worker threads, the measured entered-but-not-yet-finished segment-job count was 7–12, while the actual outstanding scheduler window was 9–15, within the configured limit of 16. The entered-job count is not a worker-thread count. RSS is informational and remains outside the acceptance gate.

Raw measurements: [raw-build-summary.json](native-segment-final-corrections/raw-build-summary.json).

## Query performance and result identity

Both matrices used three repetitions, `--query-workers 4`, verified cache `0`, and all 150 cases in ASCII and Unicode.

Repository command:

```bash
target/release/examples/bench_musenalm \
  --root examples/musenalm \
  --profiles ascii-off,unicode-off \
  --repetitions 3 \
  --query-workers 4 \
  --verified-cache-bytes 0 \
  --result-dir docs/ai/generated/native-segment-final-corrections/query-repository-final
```

Progressive command adds `--incremental` and uses `query-progressive-final`.

| Entry point | Sum-of-medians ratio | Median per-case ratio | Geometric mean ratio |
| --- | ---: | ---: | ---: |
| Repository | 1.0103 | 1.0081 | 1.0109 |
| Progressive `Index` | 1.0204 | 1.0232 | 1.0519 |

Initial sampling produced a few isolated I/O-variance cells over the investigation threshold. Targeted three-repeat runs kept the execution plans and work counters identical and returned the affected cells to baseline or low-single-digit variation, except `Q068` Unicode, which remained noisy while scanning 4,010 source files. `Q068` uses neither changed relation behavior nor inferred dictionary admission, and its selected scanner plan, counters, results, and digest are unchanged. No correction-induced material regression was found.

The repository and progressive final `result-digests.json` files both have SHA-256:

`aa907b96bb7960d361da4301aab269651285d80a3e17fcf09f9f856c8a049f63`

This matches the prior correctness-completion baseline exactly for all 150 cases, including IDs, ordering, counts, hits, and evidence.

Raw evidence:

- [Repository summary](native-segment-final-corrections/query-repository-final/REPORT.md), [measurements](native-segment-final-corrections/query-repository-final/measurements.jsonl), [digests](native-segment-final-corrections/query-repository-final/result-digests.json), [manifest](native-segment-final-corrections/query-repository-final/run-manifest.json)
- [Progressive summary](native-segment-final-corrections/query-progressive-final/REPORT.md), [measurements](native-segment-final-corrections/query-progressive-final/measurements.jsonl), [digests](native-segment-final-corrections/query-progressive-final/result-digests.json), [manifest](native-segment-final-corrections/query-progressive-final/run-manifest.json)
- [Repository outlier repeat](native-segment-final-corrections/query-repository-final-outlier-repeat/REPORT.md)
- [Progressive outlier repeat](native-segment-final-corrections/query-progressive-final-outlier-repeat/REPORT.md)

## Scope confirmation

Memory optimization, compact summaries, allocator work, replacement of native maps and Roaring postings, planner redesign, storage-backend changes, and general scheduler redesign remain deferred. The existing native maps, postings, bigram filters, segment construction, parallelism, public query semantics, and bounded resource behavior are preserved. `master` was not merged.
