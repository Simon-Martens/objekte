# Performance issue: incremental field-posting admission cost

- **Category:** `perf`
- **Originating task:** `snapshot-loading-admission-profiling`
- **Observed on:** `integration/snapshot-loading-admission-profiling`, admission instrumentation commit `93a98cc4dc578dde768c3a83e41b97d36f33e9b2`
- **Code:** `crates/core/src/mutation_segment.rs:137` (`SegmentAggregate::admit_profiled`, field-posting merge), called by `SegmentAdmission::admit_batch_profiled` at `crates/core/src/mutation_segment.rs:402` and the incremental coordinator in `crates/core/src/repository_incremental.rs:279`.

## Observation and impact

In seven profiled configured Musenalm incremental builds (148,486 files/records, 32 workers, ASCII), field-posting contribution merging took **277.09 ms median** of **395.21 ms median** exclusive admission work. The `contents` collection accounted for **258.03 ms** of field-posting merges and **368.98 ms** total admission. These are accumulated single-coordinator elapsed intervals across all 294 batches, not extrapolated worker samples. Each run's exclusive component sum plus residual equals its measured admission total; residual was at most 0.21%. The broad construction path still includes concurrent worker work and later segment sealing, so 277 ms is not a promised end-to-end saving.

The existing merge handles all declared field posting indexes, including embedded structural facts. The current behavior is correct and query digests agree across matched baseline/optimized binaries. The expected opportunity is lower wall time for this work without changing index semantics, batch ordering, publication behavior or memory limits.

## Reproduce

Use the task's [benchmark archive](../../../../bench/results/snapshot-loading-admission-profiling/README.md). Build B3 with the listed release command and run `python3 run_campaign.py` sequentially on the configured Musenalm root. Inspect `build_profile.admission.postings_ns` and `build_profile.admission_by_collection.contents.postings_ns` in `runs/construction-b3-on-*/trial-*/sample.json`; `python3 summarize.py` recomputes the medians. The full setup, binary hash, corpus fingerprints and raw trials are archived there.

## Possible investigation

Measure which field-posting families and merge operations account for this interval at a coarse per-batch or per-field-family level, then compare bounded implementation alternatives under the same correctness, memory and profile-off benchmarks. Possible causes include repeated bitmap merging, allocation and ordered-map work; these are hypotheses, not measured subcomponents. Avoid assuming that changing batch size or posting representation helps before measuring its effects on publication latency and memory.

This was left unresolved because the authorized task specifically limited admission work to attribution and excluded changing merge algorithms or posting data structures. It is a proposal for user review, not authorization to implement it.
