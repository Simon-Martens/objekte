# Musenalm index-build scheduling experiments

Starting HEAD: `07e7c66b6d837a9701cd6c97fea7897ca9a4dc9a`. The compatible reference is the fresh build after the source-buffer revert at `8387d1b039620aefce8e48f0dcac432dd5eaab42`. Core source and build example were unchanged between that recording and this task. Source and schema fingerprints match. The reference used release, rustc 1.99.1, ASCII, disabled snapshots, 32 automatic build workers, the same CPU affinity and performance governor. New scheduling counters did not exist in that recording and are unavailable for the reference. All exact commands, environment, raw JSON, stderr and schema manifests are saved here. Single-build differences are observations, not proven speedups.

| Experiment | Code SHA | Comparison SHA | Wall ms | Delta ms | Delta % | Admission ms | Finalization ms | Peak active jobs | Peak ready batches | Peak RSS MiB | Signature | Decision |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |
| Reference | 8387d1b | — | 1574.195 | — | — | unavailable | 120.062 | unavailable | unavailable | 288.766 | 52140243 | recorded |
| 1. Worker-aware allowance | 59a669a | 8387d1b | 1405.910 | -168.285 | -10.7% | 843.648 | 126.261 | 32 | 63 | 458.266 | 52140243 | retain provisionally |
| 2. Replenish between admissions | a8759e3 | 59a669a | 1485.860 | +79.950 | +5.7% | 893.628 | 137.241 | 32 | 61 | 485.922 | 52140243 | revert |
| 3. Remove build revision checks | 4c22416 | 59a669a | 1379.170 | -26.740 | -1.9% | 808.209 | 129.815 | 32 | 63 | 483.969 | 52140243 | retain provisionally |
| 5. Retain bigram blocks | d42db18 | 4c22416 | 1104.859 | -274.311 | -19.9% | 407.251 | 255.906 | 32 | 63 | 497.789 | 52140243 | retain provisionally |

Cumulative change from reference after experiment 5: -469.337 ms (-29.8%). Experiment 2 was reverted in `2c9e8c40ad` after its findings were saved; experiment 3 compares with the retained experiment 1 state.

## Experiment 1

The normal admission allowance is now `clamp(workers × 16 MiB, 52 MiB, 512 MiB)`; the 32-worker run used a 512 MiB limit and reached 499.406 MiB of **estimated admission weight**, compared with 51.406 MiB in the reference. This is a scheduling estimate, not a process-memory limit. Peak RSS rose 169.500 MiB. Peak executing jobs reached 32 and completed-but-unadmitted batches reached 63. Summed worker job time was 6827.068 ms, coordinator receive time 98.538 ms and coordinator admission/merge time 843.648 ms. These overlapping times are not parts of a wall-time sum, and receive wait alone does not show worker idleness.

The build was 168.285 ms faster than the reference sample, but the large memory increase is a material tradeoff. Retain provisionally while the next experiment tests whether faster replenishment changes the backlog. Focused scheduler and deterministic build tests passed. Discovery was 319.040 ms, extraction/index/admission 945.940 ms and finalization 126.261 ms. All seven collections contained 148,486 files and 148,486 records in total, with 148,486 successful parses and zero failed parses. Canonical registrations, schema digest and semantic index signature matched the reference.

## Experiment 2

The coordinator replenished after each admitted batch and limited speculative dispatch to one batch of the immediately following collection. Framed batches used the same admission loop. A gate-based test confirmed replacement work was dispatched before the next ready result was admitted; all focused build tests passed. The build took 79.950 ms longer than experiment 1 and peak RSS rose 27.656 MiB. Peak active jobs stayed at 32, ready backlog changed from 63 to 61, and estimated admission weight rose from 499.406 to 512.000 MiB. Coordinator admission/merge was 893.628 ms and receive time 129.028 ms; summed job execution was 5655.444 ms. Discovery was 307.359 ms, extraction/index/admission 1025.946 ms and finalization 137.241 ms. The single sample cannot prove a regression, but it shows no concurrency gain while adding scheduling complexity and memory. Revert the implementation and keep its evidence. Counts, schema, canonical registrations and semantic signature matched the reference.

## Experiment 3

Construction reads now assume stable input after discovery. The ordinary build call passes `for_build=true`, which skips the three explicit `check_revision` calls around open/read/parse; each such check previously requested opened-file and pathname metadata. Framed construction no longer checks metadata before/after reading or after range processing. Logical-validation fallback uses a build-only range reader without a metadata probe. This is a code-path audit, not an externally measured syscall count. Query-time range checks and snapshot restoration were unchanged.

Full core tests passed, including JSON/JSONL, malformed and empty sources, invalid ranges, logical fallback and oversized framed coverage. A focused test confirmed that the build reader uses actual buffer length even when the supplied revision differs. The build took 26.740 ms less than retained experiment 1; peak RSS rose 25.703 MiB, so timing and memory changes remain single-run observations. Discovery was 327.089 ms, extraction/index/admission 907.953 ms and finalization 129.815 ms. Active jobs and ready backlog remained 32 and 63; admission/merge was 808.209 ms, receive time 96.776 ms, and summed job execution 6165.163 ms. Estimated peak admission weight stayed 499.406 MiB under the 512 MiB limit. Counts, schema, canonical registrations and semantic signature matched.

## Experiment 5

Worker chunks now retain gram words, and ordered admission translates block starts without expanding every membership. Finalization converts owned normal and skip columns to the existing Roaring format on the build pool. The published index and snapshot layout are unchanged. Focused fixtures compared complete normal/skip postings and candidates with the former incremental merge across both alphabets, record counts 63/64/65 and 511/512/513, nonaligned batch bases and the 65,536 bitmap boundary. Full core tests, including snapshot round trips, passed.

The build took 274.311 ms less than experiment 3. Admission/merge fell 400.958 ms to 407.251 ms, while finalization rose 126.091 ms to 255.906 ms because conversion now occurs there. **Total repository-opening time**, not the moved phase cost, is the comparison. Peak RSS rose 13.820 MiB to 497.789 MiB. Discovery was 321.511 ms and extraction/index/admission 513.349 ms; summed job execution was 5392.460 ms and receive time 102.929 ms. Peak active jobs (32), completed-but-unadmitted batches (63), and estimated admission weight (499.406 MiB under a 512 MiB limit) were unchanged. Counts, schema, canonical registrations and semantic signature matched.

## Recommendation

Retain experiments 1, 3 and 5 provisionally and leave experiment 2 reverted. The final single build was 469.337 ms faster than the starting reference, with peak RSS 209.023 MiB higher. The worker-aware allowance accounts for most of that memory increase; the block representation adds another 13.820 MiB relative to experiment 3. These samples do not establish repeatability or isolate allocations, but the block result is large enough to justify keeping its reversible commit for now. No query, restore, profiling, worker-count sweep or additional final build was run.
