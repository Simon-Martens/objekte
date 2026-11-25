# Objekte ingestion follow-up merge review

Reviewed 25 November 2026. Branch `integration/ingestion-repair-followup`. Implementation `b2feb41c01398dc2cfb57d3ec93726e7b3943917`. Report and branch tip `2a748ed285ced6d5980f2cfd4e823a4295bf3c98`. Parent under review `02cf0a1b2bedf5893e05204c90ade0d55f30c9c5`.

**Decision. Keep the follow-up and hold the merge for the registered-build lifecycle, retained contributions and segment identity corrections below.** Preserve the measured small-file throughput improvement. The fourth item is a construction scaling issue that matters for the upcoming JSONL extraction. The final note applies to an optional discovery mode.

## Verified progress

| Previous finding | Evidence on this branch |
|---|---|
| One publication per registered file | The 8,192-file fixture now has 128 content publications and 128 segments of 64 in each of three runs. Median build time changed from 858.871 ms to 167.834 ms. The gated tests also exercise publication of an independent group while a large or shared binding is paused. |
| Whole-document range partitioning | The whole-document work partitioner now uses the record target. Tests check 4/4/2 and 100/100/100/10 through initial build, managed add and conditional replacement. |
| Verified update holds the writer lock | Old-content reconstruction, native finalization, private compaction and fallback verification precede the installation lock. The new gated tests require an unrelated direct publication to complete during these phases. |
| Unaccounted concurrent framing | The implementation takes shared framing permits and measures simultaneous pending and admitted input. The registered callback retains control of its own allocations before it emits. Retained contribution objects in the new packing path are a separate issue described below. |
| Symlink eligibility | Initial publication checks the discovery policy and object identity. The regression covers a changed intermediate symlink and a competing alias. |

I recalculated all six pairs of workload medians from the 36 checked-in observations. All 36 commands exited successfully. The nine follow-up single-source samples each report one content open, the complete 100,000 records in 25 segments, and the expected final byte span. The committed workspace output reports 726 passing core unit tests and 26 passing ingestion integration tests. Both release stress logs pass. The findings below are from tracing the pushed source. Cargo and rustc were unavailable for an independent native run.

[Implementation report](https://github.com/Simon-Martens/objekte/blob/2a748ed285ced6d5980f2cfd4e823a4295bf3c98/docs/ai/done/objekte_ingestion_repair_review_and_followup.md) · [Raw samples](https://github.com/Simon-Martens/objekte/blob/2a748ed285ced6d5980f2cfd4e823a4295bf3c98/docs/ai/generated/ingestion-repair-followup/comparison.jsonl)

## 1. Make registered-group cancellation and failure terminal only after outstanding work is drained

**Failure sequence.** In `run_stream_groups`, the coordinator checks cancellation, blocks in `receiver.recv()`, accepts the result, and publishes a completed group. A caller can cancel while that receive is blocked. If the last result for a group then arrives, the group can become searchable before the coordinator next checks cancellation. The publication closure seals tails and calls `publish_initial_segments` without another cancellation check. Separately, a worker error, cancellation, or publication error returns immediately from this loop. Other files were submitted with detached `pool.spawn` and their staging closures have no cancellation predicate. The background build can report its terminal error while those callbacks still run.

**Correction.** Keep ownership of every dispatched file job until completion. Stop dispatch on the first error or cancellation, make already-running jobs stop at cooperative chunk/binding boundaries, and drain their completion signals. Preserve the first meaningful error. Give group installation a cancellation decision point after private preparation and immediately before committing. Define the race with a concurrent installation so a completed cancellation request before that decision suppresses the candidate. Release all private segments, frame permits and staged tails. Callbacks already inside application code can return cooperatively.

**Acceptance.** With the public progressive build and two independently publishable groups, gate the last callback of one group after the coordinator is waiting. Call `cancel()`, then release the worker. The cancelled group must never appear in any pinned or later view. A group visible before cancellation must survive. For failure, let one reader return a fatal error while a sibling callback is held. Observe that terminal completion waits for sibling exit. Always release test gates, and verify zero outstanding callbacks and permits after `wait_complete()` returns.

[Registered group coordinator](https://github.com/Simon-Martens/objekte/blob/2a748ed285ced6d5980f2cfd4e823a4295bf3c98/crates/core/src/construction/initial.rs#L237) · [Group installation](https://github.com/Simon-Martens/objekte/blob/2a748ed285ced6d5980f2cfd4e823a4295bf3c98/crates/core/src/construction/initial.rs#L1539)

## 2. Bound staging for valid chunks containing zero record ranges

**Failure sequence.** `stage_stream_binding` pushes every decoded `BatchContribution` into `tail`. It drains that vector only when the accumulated record count reaches the segment target. The registered-reader contract permits chunks that consume input and emit no record ranges. A reader that skips headers, comments or blank lines can emit arbitrarily many such chunks. Every chunk produces a batch with a file contribution and zero records. On the multi-file registered path, staging can therefore retain one intermediate batch per chunk until EOF, while the decoder credits for those batches have already been released. This is growth in core-owned objects for an input whose actual record count can remain zero.

**Correction.** Aggregate empty contributions as they arrive. Preserve the file's successful participation, status, diagnostics, observed fingerprint and any meaningful accounting. Keep enough information to publish a valid zero-record file and to pack a later nonempty tail. Bound actual retained intermediate work independently of the number of zero-range emissions. Distinguish private completed segments and unsealed contributions in the test counters.

**Acceptance.** Use at least two registered physical files so the test selects `run_stream_groups`. Have one reader produce successively larger counts of consecutive, nonempty byte chunks with zero ranges. Run one case with no records and another with one record at EOF. Assert bounded retained contribution count as the number of chunks increases, one successful file membership, correct reverse lookup and correct final byte range. Repeat a late failure to verify that no candidate for that file publishes.

[Tail retention](https://github.com/Simon-Martens/objekte/blob/2a748ed285ced6d5980f2cfd4e823a4295bf3c98/crates/core/src/construction/initial.rs#L112) · [Reader contract checks](https://github.com/Simon-Martens/objekte/blob/2a748ed285ced6d5980f2cfd4e823a4295bf3c98/crates/core/src/construction/file_preparation.rs#L901)

## 3. Give query providers the published segment identity

**Failure sequence.** Each independent managed file placement prepares its first segment with provisional ID zero. The new reusable-placement path gives its published wrapper a unique ordinal, leaving the immutable payload's provisional ID intact. Both the structured and text query providers still build their slot-domain identity from the payload ID. Two one-record managed additions to the same collection can consequently produce the same provider domain for different published segments in the same view. This violates the slot-domain identity contract and can mislabel execution diagnostics. The reviewed source establishes the collision. It does not establish an incorrect set of search hits.

**Correction.** Pass the published segment ID or a published read view into the query providers and any executed-observation paths consuming a managed segment. Audit callers that build external-facing component descriptors from a payload. When describing a published segment, report the wrapper's identity. Keep prepared payloads shareable across retries.

**Acceptance.** Add two one-record files independently to the same already-built collection. Record the distinct IDs from the pinned view's segment summaries. Run a structured filter and a text query over both files with execution observations enabled. Assert that observations for the two physical segments carry the corresponding distinct domains, and that both queries return the expected IDs. Add an internal assertion that an input domain from one segment cannot validate against the other. Include a replacement or compaction and recheck a pinned older view.

[Placement](https://github.com/Simon-Martens/objekte/blob/2a748ed285ced6d5980f2cfd4e823a4295bf3c98/crates/core/src/lifecycle/mutation/staging.rs#L16) · [Published identity](https://github.com/Simon-Martens/objekte/blob/2a748ed285ced6d5980f2cfd4e823a4295bf3c98/crates/core/src/lifecycle/publication.rs#L580) · [Structured provider](https://github.com/Simon-Martens/objekte/blob/2a748ed285ced6d5980f2cfd4e823a4295bf3c98/crates/core/src/query_access/segment.rs#L66) · [Text provider](https://github.com/Simon-Martens/objekte/blob/2a748ed285ced6d5980f2cfd4e823a4295bf3c98/crates/core/src/query_access/text.rs#L63)

## 4. Preserve parallel decoding when a second registered file is present

A single registered source gets the standalone scheduling path. Once two registered physical files are discovered, the group scheduler dispatches file callbacks on pool workers and caps concurrent file jobs at four. The registered decoder runs inline whenever its caller is already on a pool worker. Consequently, one large registered file plus one tiny registered file can leave the large file decoding one chunk at a time after the tiny one finishes, even with eight or 32 workers available. The 100,000-record benchmark measured a single physical file. The many-file benchmark measured one record per file. Neither measures this combination. The limit here concerns concurrent calls to the record decoder; native segment finalization can have additional parallel work.

Use the existing pool and shared byte budget to make spare decoder workers available across file groups while retaining the progress guarantees at one worker. Compare the same 100,000-record registered input alone and with a tiny registered neighbor. Record per-file simultaneous decoder calls, source opens, locators, segment populations and wall time. With several workers and enough independent chunks, require overlapping decoding within the large file even when the small neighbor is present. Continue to permit unordered callbacks and preserve atomic file visibility.

[Multi-file dispatch](https://github.com/Simon-Martens/objekte/blob/2a748ed285ced6d5980f2cfd4e823a4295bf3c98/crates/core/src/construction/initial.rs#L1006) · [Inline decoding decision](https://github.com/Simon-Martens/objekte/blob/2a748ed285ced6d5980f2cfd4e823a4295bf3c98/crates/core/src/construction/file_preparation.rs#L739)

## Optional-mode scaling note

With `FollowWithinRoot`, the initial verifier runs full discovery for each distinct selection in each content publication. For an 8,192-file selection and 128 groups, this implies 128 complete selection traversals in addition to initial discovery, even if the tree is stable. The default `Skip` policy avoids this cost. Before using follow mode on a large collection, measure directory visits and preserve deterministic alias selection if changing the strategy. This observation is independent of the small-file measurements above. [Initial eligibility verification](https://github.com/Simon-Martens/objekte/blob/2a748ed285ced6d5980f2cfd4e823a4295bf3c98/crates/core/src/lifecycle/publication.rs#L1084)

## Completion

Use the reviewed branch tip as parent. Add the focused regression for each affected path. Preserve the existing segment geometry, mixed ingestion, verified-writer progress and public visibility fixtures. Run the locked workspace tests, existing public mixed-ingestion example, and single-file, many-small-file and mixed-size registered workloads against the final implementation. Supply the pushed SHA, actual high-water counts and the observed terminal states. Microsecond changes in add or move are informational.


## Implementation report

Completed 2026-11-25 on `integration/ingestion-followup-merge-review` in `/home/simon/source/objekte-integration-ingestion-followup-merge-review`. The reviewed parent is `2a748ed285ced6d5980f2cfd4e823a4295bf3c98`. Implementation and benchmark revision `d2ae68aeb7e1d24673b4ce0f622c8fa417b8101d` is pushed for review.

| Review item | Correction and regression result |
| --- | --- |
| Terminal cancellation and worker draining | The registered scheduler owns scoped jobs. Small-only builds and one-worker builds use the repository pool. Large multi-worker builds use a bounded set of framing coordinators. It stops dispatch at the first observed cancellation or error, checks cancellation at read, chunk and binding boundaries, and drains dispatched work before completing. The last cancellation check runs inside the publication write lock immediately before the swap. `registered_cancellation_keeps_prior_group_and_drains_active_callback` observed `Cancelled`, the prior visible group at the same revision, and zero outstanding work. `registered_fatal_completion_waits_for_held_sibling` observed that a fatal callback did not make the build terminal while its sibling remained held. The final `Failed` result retained the fatal reason and all callback and permit counts returned to zero. |
| Zero-range staging | Empty chunks advance offsets and framing counts and contribute to the observed file digest while bypassing decoder and tail construction. An entirely empty successful source still contributes one file entry. `registered_empty_frames_keep_constant_unsealed_work` measured at most two unsealed contributions across both files and a peak of one private segment at both 64 and 30,000 zero-range frames. The 30,000-frame cases cover zero records, a record at `[30000,30013)`, and a late fatal result. Reverse lookup and file participation pass. The fatal file has no published segment. |
| Published query domains | Structured and text providers, relation and facet callers, executed observations and component descriptors use the wrapper's ordinal. Native payloads remain shareable across placement retries. `managed_queries_and_components_use_distinct_published_domains` observes the two expected domains and exact hits in both query types. Each provider rejects input from the other segment. The test repeats the observations and hit checks on the replacement revision and on the pinned older view. |
| Parallel large-file decode with a peer | For large grouped builds, framing callbacks run on bounded coordinators while decoding uses the existing repository pool and shared byte credits. Large grouped files leave decoder capacity for a peer. `large_registered_file_decodes_concurrently_beside_a_small_file` held two decoder calls from the same large file and observed the small file alone in a searchable revision before releasing them. The release fixture below measures the same 100,000-record stream with and without a tiny neighbor, including one-worker completion. |

### Validation

`cargo fmt --all -- --check`, `cargo check --locked --workspace --all-targets`, `cargo test --locked --workspace`, and `cargo run --locked -p objekte-search-core --example record_ingestion_public` passed. The workspace output includes 731 passing core unit tests and 26 passing record-ingestion integration tests. The two release stress fixtures below passed. [Workspace results](../generated/ingestion-followup-merge-review/workspace-tests.txt) · [Gated regressions](../generated/ingestion-followup-merge-review/focused-tests.txt) · [Release stress](../generated/ingestion-followup-merge-review/release-stress.txt)

```bash
cargo test --locked --release -p objekte-search-core --lib spool_overflow_accepts_beyond_resident_descriptor_window -- --ignored --nocapture
cargo test --locked --release -p objekte-search-core --test record_ingestion routed_size_filter_stays_selective_after_managed_catalog_churn -- --ignored --nocapture
```

### Paired release benchmarks

Linux-7.3.3-200.fc44.x86_64-x86_64-with-glibc2.43 with 32 available CPUs, governor `performance`, rustc 1.99.1 (58cf8f9c3 2026-10-13) and cargo 1.99.1 (5f94df478 2026-08-27). The paired runner used separate Cargo target directories to build the same fixture on the reviewed and repaired revisions with `--locked --release --features detailed-profiling`. The baseline probe recorded mixed-file concurrency without enforcing the repaired overlap assertion. Times, peak RSS, and decoder counts are medians of three alternating pairs. [Runner](../generated/ingestion-followup-merge-review/compare.py) · [Manifest](../generated/ingestion-followup-merge-review/comparison-manifest.json) · [Raw observations](../generated/ingestion-followup-merge-review/comparison.jsonl)

| Fixture | Wall time, reviewed → repair | Peak RSS, reviewed → repair | Per-file active decoders, reviewed → repair |
| --- | ---: | ---: | ---: |
| JSONL alone, 32 workers | 56.233 → 56.830 ms | 63.0 → 63.0 MiB | — |
| Registered alone, 1 worker | 278.355 → 279.063 ms | 54.7 → 54.8 MiB | 1 → 1 large |
| Registered alone, 8 workers | 90.365 → 93.024 ms | 56.2 → 56.1 MiB | 5 → 4 large |
| Registered with a tiny neighbor, 1 worker | 261.930 → 268.710 ms | 55.2 → 55.4 MiB | 1 → 1 large, 1 → 1 tiny |
| Registered with a tiny neighbor, 8 workers | 266.839 → 107.626 ms | 55.8 → 57.6 MiB | 1 → 4 large, 1 → 1 tiny |
| 8,192 small registered files, 8 workers | 170.020 → 180.113 ms | 26.3 → 26.5 MiB | — |

The 100,000-record file used one content open and produced 24 segments of 4,096 plus one of 1,696. Its last record spanned `[29999700,29999999)`. Mixed runs used two content opens and two installations, preserving the same large-file geometry. The tiny file added one record with the locator `[0,22)`. The many-file workload preserved 128 segments of 64 and 128 content installations.

Keeping small-only groups on the repository pool recovered most of the first implementation's slowdown. The final small-file wall-time samples overlapped, with 161.3 to 191.8 ms on the reviewed revision and 167.0 to 191.5 ms on the repair. The mixed large file overlapped four to six of its own decoder calls in every repaired eight-worker sample.

Cancellation is cooperative inside application callbacks. The build waits for an entered callback to exit before reporting a terminal result. The optional follow-within-root policy still performs alias discovery per distinct selection and publication group, as called out in the review.
