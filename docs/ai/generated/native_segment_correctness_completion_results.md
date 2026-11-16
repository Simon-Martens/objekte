# Native segment correctness completion results

Task identifier: `native-segment-correctness-completion`

## Revision and environment

- Starting revision: `35ea1b83bd5fa08fd321e900b563a473ef12715f`
- Final tested implementation revision: `93210b976fedd45d9f1b1ea4e8eeb293a3b1c21d`
- Branch: `integration/native-segment-construction-repair`
- Integration worktree: `/home/simon/source/objekte-integration-native-segment-construction-repair`
- Toolchain: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`
- `Cargo.lock` SHA-256: `dc29ff42cf24b2cc4de67a2297f3c4d676c9034c0b91315465d1fb1136b486f7`
- Query benchmark binary SHA-256: `b579e4523e4d3d564d3d0ef78e94ed700a3f4ba563e7da5398dbb676e12af9b3`
- Construction benchmark binary SHA-256: `f9b05ba8349022815d51719dcb478d6345ba23623c8abb58193397e545758eac`
- Musenalm corpus fingerprint: `b79783c538f9fb76b284cc1535a36d4c9864d5d4d4728e698704eaea9ef0b5fd`

Implementation commits:

| Commit | Scope |
| --- | --- |
| `68c072894b` | Qualified native relation membership, pinned-view conversion, both semijoin strategies, relation tests, and removal of unused flat streaming prefetch |
| `17395194a5` | Segment inference observations, two-pass aggregation, shared semantic admission, capability diagnostics, and inference regressions |
| `93210b976f` | Private framed-file finalization through the segment scheduler, bounded completion window, atomic publication, invalidation/cancellation/error tests |

The results report commit is intentionally later than the tested implementation revision.

## C01-C07 completion

| ID | Implementation | Exact regression evidence | Result |
| --- | --- | --- | --- |
| C01 | `run.rs::try_semijoin_relation_page` carries `SegmentRecordSet` from source candidates through qualified FTS witnesses; results are formed directly from qualified addresses | `relation_semijoin::tests::relation_semijoin_distinguishes_repeated_local_slots_across_segments`, `relation_fts_counts_distinct_local_slots_in_three_segments_once` | Complete |
| C02 | `relation_semijoin.rs` validates terminal targets per stable `(CollectionId, SegmentId)` and intersects restrictions in the target segment's local domain | `relation_target_match_in_second_segment_is_not_redirected_to_first`, `both_physical_strategies_keep_independent_witnesses_and_reuse_targets` | Complete |
| C03 | The unused segmented streaming flat prefetch was removed; merged-view target reuse remains active | `relation_target_cache::tests::segmented_stream_skips_unused_flat_prefetch_allowance`; selected relation streaming in the C01 test with cache enabled and disabled | Complete |
| C04 | `InferenceObservation` is captured before builder finalization and collection aggregation is a deterministic two-pass operation over retained segment observations | `global_threshold_admits_a_field_when_each_segment_is_locally_below_it`, `empty_ancestor_reconciles_late_descendant_and_null_statistics`, `mutation_keeps_query_parity_with_construction_generation_observations` | Complete |
| C05 | The single `decide_field_admission` path drives single-domain and collection-wide decisions; reports expose truncated summaries and component capabilities | `global_distinct_limit_rejects_dictionaries_locally_admitted_in_both_segments`, `repeated_percent_boundary_is_inclusive_and_exact`, `segmented_observation_overflow_uses_residual_scanning` | Complete |
| C06 | Sealed large-file segments are submitted immediately through a private scheduler destination, consumed within `2 * workers`, and published in one revision after file validation | `large_file_finalizes_private_segments_before_eof`, `private_segment_window_is_bounded_for_one_and_multiple_workers`, `late_private_invalidation_and_worker_failure_publish_nothing`, `private_work_cancels_while_outstanding_and_after_validation` | Complete |
| C07 | This report and exact-commit raw benchmark artifacts are committed with the completed task document | This file; raw links below | Complete |

## Relation defects and execution-path evidence

The reviewed baseline flattened each segment's local slot into one collection bitmap before invoking the relation engine. Thus local slot `0` in two segments became one address. The repaired path keeps:

```text
source candidates:
  (CollectionId, SegmentId) -> local RoaringBitmap

target candidates and exact matches:
  same qualified domain

reverse witnesses:
  same qualified domain, intersected with the original source input
```

`MergedRelationView` owns a checked stable-segment-to-view-position map. A `SegmentRecordAddress` is rejected if its slot is outside the pinned segment record or active domain. Genuine legacy base addresses remain `None`; they are not assigned synthetic native segment IDs.

Execution evidence:

- Repeated slot `0` in two source segments: both related FTS and structured relation queries return only the segment-aware witness `S0`; source filtering for `S1` returns `S1` only for its target text.
- Forced structured target-first and source-restricted strategies return the same explicit logical result on the target-in-second-segment fixture.
- A three-segment fixture with fresh local slots `0` and `1` returns each matching logical source exactly once. Relation FTS reports `count = 5`, `scanned_records = 0`, and target evidence on `entry.title`.
- The relation lifecycle test performs add, replace, delete, move, verified cache use, and compaction. Locator and Bytes result modes agree with the reference evaluator after every revision.
- Streaming with and without verified-result caching returns the collision witness once and stays within one target file open/read for the small target.
- The source-dependent flat prefetch call was removed from segmented streaming. The merged relation evaluator's target projection cache remains in use.

Raw test output: [relation tests](native-segment-correctness-completion/relation-tests.txt).

### Reviewed-baseline replay

Temporary public-API integration tests were compiled in a detached worktree at `35ea1b83bd5fa08fd321e900b563a473ef12715f`. The reproducers failed on the reviewed source:

- Repeated related-FTS slot `0` across two source segments returned no witness (`[]`) instead of the expected `["S1"]`.
- A late `late` field observed only in the final `1`-record segment reported `eligible_records = 1` and `missing_records = 0` instead of the collection-wide `eligible_records = 513` and `missing_records = 512`.

The baseline test process exited `101`; the preserved output is [baseline-reproducers.txt](native-segment-correctness-completion/baseline-reproducers.txt). The detached baseline worktree was removed after the output was copied into this branch.

The C06 before-EOF behavior can only be observed through the private scheduler/gate boundary, so its reproducer is a current-branch unit test rather than a public-API baseline test. The reviewed implementation built the complete `private_segments` vector before publication; the current gate test proves that finalization now completes while EOF validation is paused.

## Inference observations and admission

Each indexed segment now retains an `InferenceObservation` independent of its candidate posting maps. The observation includes logical and unavailable counts, path shape/kind bitmaps, direct and through-array categories, null/empty-selection facts, exact bounded distinct sets, short-string coverage, truncation state, and omitted-path count.

Aggregation is deterministic and two-pass:

1. Union observed paths and collection counts.
2. Visit every segment for every path, including segments where the path was absent.
3. Apply retained empty-ancestor bitmaps for late nested paths.
4. Merge bounded distinct sets and preserve overflow state.
5. Pass the aggregate through the same `decide_field_admission` function used by a single-domain builder.

Component diagnostics now distinguish complete value membership, short-string candidates, exact empty-string membership, array length, and structural facts. Lifecycle publication applies the collection decision to each physical segment: rejected dictionaries lose value proofs, while permitted structural, empty-string, and length capabilities remain. A complete all-short dictionary remains usable even when the general long-string repetition rule does not admit a general dictionary.

Evidence:

- `513 = 512 + 1` records cross the global nonblank occurrence/record minimum together; the locator query reads and parses zero source records.
- Repeating with minimum `514` rejects general dictionary proof and falls back to source verification with reference-identical results.
- `65` disjoint strings reject at maximum `64`; `64` strings admit. Overflow is explicitly marked instead of reporting zero as an unknown count.
- The repeated-percent boundary admits exactly `80%` and rejects `81%`.
- Segment-local observation overflow reports incomplete coverage and uses residual scanning.
- ReportOnly produces the same late-field and nested empty-selection statistics without candidate posting maps.
- Direct/array mixed shape, null-plus-non-null overlap, late field, late nested field, and overlapping short-string sets are asserted on the actual `512 + 1` segment distribution.
- Mutation and compaction queries remain reference-identical. The API documents the report as construction-generation data and compaction/rebuild paths mark the carried report incomplete.

Global memory accounting across all retained segments remains deferred. Local drop flags and local bounds are preserved; this task does not claim that `max_repository_bytes` is a process-wide retained-memory enforcement mechanism.

## Framed-file overlap and atomicity

Large framed files now:

1. Seal and drain the preceding ordinary segment work.
2. Submit each sealed private segment through the existing segment scheduler.
3. Keep at most `max(1, 2 * build_workers)` private completions outstanding.
4. Consume completed indexed payloads into an ordered private group.
5. Publish every segment for the validated physical file in one revision.

The test-only gate proves that a private segment finishes while the producer is paused before final EOF validation. The catalogue remains visible, no private segment is searchable, and all records appear atomically after release.

The one- and three-worker `9 MiB` JSONL fixtures each produce more than `W + 3` segments. Tests assert one publication, complete record count, `peak_segment_window <= 2P`, and `peak_segment_jobs <= P`.

Late whole-file invalidation after a completed private segment, cancellation with work outstanding, and cancellation after validation all publish zero segments and zero inference observations. Injected private worker failure drains every outstanding completion without deadlock and publishes nothing.

Raw test output: [framed-file pipeline tests](native-segment-correctness-completion/framed-file-tests.txt).

## Validation

Final results at the tested implementation revision:

| Command | Result |
| --- | --- |
| `cargo test -p objekte-search-core --lib --locked` | 538 passed, 16 ignored, 0 failed |
| `cargo test -p objekte-search-core --tests --locked` | All integration and doctest targets passed |
| `cargo test --workspace --locked --no-fail-fast` | All workspace unit, integration, and doctest targets passed; ignored tests remained ignored |
| `cargo check --workspace --all-targets --locked` | Passed |
| `cargo clippy --workspace --all-targets --locked` | Passed; repository-pre-existing warnings remain |
| `cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked` | Passed |
| `git diff --check` | Passed |

The ignored tests are the pre-existing manual timing, large-corpus acceptance, release-only memory, and scale tests. The clippy run reports existing workspace warnings, including pre-existing advisory lints outside this change; it is not warning-free baseline code.

## Construction performance

Command, run three times per row:

```bash
target/release/examples/bench_build_musenalm \
  --root examples/musenalm \
  --alphabet <ascii|unicode> \
  --build-workers <1|8>
```

| Workers | Alphabet | Baseline median | Final median | Change | Final peak RSS |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | ASCII | 3606.076 ms | 3547.476 ms | -1.6% | 481.169 MB |
| 1 | Unicode | 4345.453 ms | 4294.997 ms | -1.2% | 488.960 MB |
| 8 | ASCII | 800.812 ms | 814.378 ms | +1.7% | 526.090 MB |
| 8 | Unicode | 923.949 ms | 899.332 ms | -2.7% | 528.863 MB |

RSS is informational and not a completion gate. All three repetitions retained the expected `15` native segments and stable index signatures within each alphabet.

Observed peak concurrent segment jobs were `1` at one worker and `8-10` at eight workers. The corresponding peak scheduler windows were `2` and `9-11`, below the configured `2 * workers` window. The complete per-run values are in the linked raw JSON.

Raw final measurements:

- [1 worker ASCII 1](native-segment-correctness-completion/raw/final-build-w1-ascii-r1.json), [2](native-segment-correctness-completion/raw/final-build-w1-ascii-r2.json), [3](native-segment-correctness-completion/raw/final-build-w1-ascii-r3.json)
- [1 worker Unicode 1](native-segment-correctness-completion/raw/final-build-w1-unicode-r1.json), [2](native-segment-correctness-completion/raw/final-build-w1-unicode-r2.json), [3](native-segment-correctness-completion/raw/final-build-w1-unicode-r3.json)
- [8 workers ASCII 1](native-segment-correctness-completion/raw/final-build-w8-ascii-r1.json), [2](native-segment-correctness-completion/raw/final-build-w8-ascii-r2.json), [3](native-segment-correctness-completion/raw/final-build-w8-ascii-r3.json)
- [8 workers Unicode 1](native-segment-correctness-completion/raw/final-build-w8-unicode-r1.json), [2](native-segment-correctness-completion/raw/final-build-w8-unicode-r2.json), [3](native-segment-correctness-completion/raw/final-build-w8-unicode-r3.json)

Immediate baseline construction artifacts are under [the repair evidence directory](native-segment-construction-repair/raw/).

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
  --result-dir docs/ai/generated/native-segment-correctness-completion/query-repository
```

Progressive command adds `--incremental` and uses `query-progressive` as the result directory.

| Entry point | Sum-of-medians ratio | Median per-case ratio | Geometric mean ratio | Material outliers |
| --- | ---: | ---: | ---: | --- |
| Repository | 1.0066 | 0.9999 | 1.0043 | None |
| Progressive `Index` | 0.9972 | 0.9868 | 0.9630 | None |

No query crossed the required regression gate of more than `20%` slower and at least `1 ms` slower. The three-sample nearest-rank p95 is the maximum sample, so collected tails are retained as raw data rather than presented as stable tails.

All four baseline/final result-digest files have SHA-256:

`aa907b96bb7960d361da4301aab269651285d80a3e17fcf09f9f856c8a049f63`

Thus IDs, ordering, counts, hits, and evidence digests are identical between Repository and progressive execution and between the immediate repair baseline and this implementation.

Raw evidence:

- [Repository run manifest](native-segment-correctness-completion/query-repository/run-manifest.json), [summary](native-segment-correctness-completion/query-repository/REPORT.md), [measurements](native-segment-correctness-completion/query-repository/measurements.jsonl), [result digests](native-segment-correctness-completion/query-repository/result-digests.json)
- [Progressive run manifest](native-segment-correctness-completion/query-progressive/run-manifest.json), [summary](native-segment-correctness-completion/query-progressive/REPORT.md), [measurements](native-segment-correctness-completion/query-progressive/measurements.jsonl), [result digests](native-segment-correctness-completion/query-progressive/result-digests.json)

## Representative inference workload

The fixture has `4,096` JSONL records, a declared `manual` field, an auto-index-prohibited `blocked` field, and inferred `kind` and `flag` fields. Its source SHA-256 is `b121be76dbfc1d8c592f8221e6e1d1cf71ef0032d47fe8df78247cf1e9887915`.

| Mode | Median open/build | Median build time | Parses | Complete | Admitted inferred paths |
| --- | ---: | ---: | ---: | --- | ---: |
| AutoIndex | 15.869 ms | 15.817 ms | 4,096 | yes | 2 |
| ReportOnly | 18.072 ms | 18.016 ms | 4,096 | yes | 0 |

Raw runs: [AutoIndex](native-segment-correctness-completion/inference-auto-r1.json), [ReportOnly](native-segment-correctness-completion/inference-report-r1.json), with repetitions `2` and `3` in the same directory.

## Deferred work

- Whole-segment cloning and temporary sorting/metadata compatibility copies remain.
- RSS reduction, compact summaries, and allocator work remain deferred.
- Repository-wide inference memory-cap accounting across live extraction and retained segments remains deferred.
- Full R07 public low-level API and snapshot migration remains deferred except the narrow pinned-view adapters required for relation correctness.
- Full R08 independent scheduling of every custom/text/index component remains deferred; measured scheduler behavior was preserved.

These are scope decisions, not accidental blockers. The original broader repair specification retains its other deferred items and is not relabeled complete by this task.
