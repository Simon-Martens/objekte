# Objekte: measure index construction on master

Prepared: 12 November 2026  
Task identifier: `index-construction-profiling`  
Intended task path: `docs/ai/tasks/objekte_index_construction_profiling_specification.md`

## 1. Assignment and scope

Measure where construction time and memory are spent in the existing single-pass implementation on master. Deliver a trustworthy breakdown and a ranked list of evidence-backed opportunities before implementing optimizations.

The user explicitly authorizes this profiling task directly on master. Preserve unrelated work, record the actual starting revision, commit only task-owned instrumentation/benchmark/report changes, and push after each commit according to repository policy. Do not merge or cherry-pick `integration/staged-index-construction`, delete that branch, or reintroduce its architecture.

Inspected master: `b19df1ed4a38c10c879fa3b0c72b1650a0dab203`. Recheck it on execution. Read current `AGENTS.md` and `docs/ai/README.md`; the direct-on-master instruction is the explicit exception for this task.

This assignment authorizes instrumentation and measurements only. Preserve indexing policy, validation, query semantics, construction scheduling, compression, and persistence behavior. No new indexing passes, background jobs, partial publication model, compact field-state representation, or production optimizations belong in this task.

Use the user's real Musenalm dataset and usual benchmark machine. Preparing this specification did not run a new benchmark: the originating session could inspect GitHub but did not have the user's checkout, dataset, or Rust toolchain.

## 2. Existing evidence and instrumentation

The archived predecessor benchmark on the inspected master commit already records:

| Exclusive coarse interval | ASCII | Unicode |
| --- | ---: | ---: |
| Discovery | 676.277 ms | 650.811 ms |
| Combined extraction and index construction | 1,666.109 ms | 1,373.104 ms |
| Per-collection finalization | 429.718 ms | 428.945 ms |
| Difference between total build and the above | 129.253 ms | 121.991 ms |
| Total build | 2,901.358 ms | 2,574.851 ms |
| Snapshot writing, outside build | 355.222 ms | 353.762 ms |

These are the recorded build samples from the archived suite, not new seven-run medians. Do not combine their phase durations with the separately measured 2.552-second paired median.

Evidence:
- `bench/results/staged-index-construction/baseline-suite/index-results.json` at `bee59df5`.
- The adjacent `run-manifest.json` identifies the measured code as `b19df1ed4a38...`, corpus fingerprint `259b435df73c0eb2d1d4f066f1528f9dc63c14b29766d19798078fdcd569b473`, Rust 1.99.1, and the actual command.
- These files are reference evidence on the unmerged branch; do not merge that branch to obtain them.

Reuse existing structures:

- `repository.rs::Repository::build_fresh` and `StartupStats`: coarse discovery, extraction/index, finalization, worker, admission, and inference counters.
- `build.rs::BuildScheduleStats`: job elapsed time, coordinator receive wait, admission time, concurrency and queue peaks.
- `build.rs::BatchBuilder::{logical, fallback, finish}`: record preparation, postings/inference, relations, text/bigrams, sort keys, batch compression.
- `build.rs::process_repository_file_batches` and framed/JSONL paths: bounded scheduling and source processing.
- `crates/core/examples/bench_musenalm.rs`: existing build and snapshot measurements, manifests, memory estimates, and query checks.

The combined extraction timer includes preparation, worker execution, coordinator admission, and fingerprint assembly. It cannot currently tell us that parsing or field indexing is the dominant cost.

The finalization timer ends before global relation resolution/validation, sort-status refresh, diagnostics validation, pool teardown, and some repository setup. Pool construction also sits outside the three coarse intervals. Instrument these explicitly; do not label the entire residual difference as relation time without measurement.

Existing `job_execution_ns` sums elapsed durations across workers. It is not CPU time and must not be presented as an exclusive fraction of wall time.

## 3. Measurement design

Produce two complementary views:

1. **Elapsed construction timeline:** nonoverlapping top-level intervals whose sum reconciles with observed end-to-end build time.
2. **Work attribution:** worker elapsed durations, actual CPU samples/time, I/O observations, allocation costs, and coordinator work. Show their overlap rather than adding them into a false 100% pie chart.

Nested timers must be labeled inclusive or exclusive. Account for their overhead and keep a visible unattributed remainder. Explain the timer boundaries in the report.

Add an opt-in build-profiling mode through the existing benchmark/configuration conventions. Normal builds must retain negligible overhead. Avoid clock reads, locks, logging, and global atomic increments per character, gram, field occurrence, or allocation.

Use worker-local/batch-local counters and aggregate at existing batch boundaries. For fine record-level measurements, sample batches/records or use a separate sampling-profiler run. Do not force single-thread execution to obtain timings and then describe them as the production parallel profile.

Where parsing, validation, and extraction are fused, report the combined measured stage and use CPU samples to identify its components. Do not duplicate or restructure production processing merely to manufacture clean timer categories.

## 4. Required breakdown

| Area | Separate observations |
| --- | --- |
| Configuration/setup | Schema and adapter preparation, worker-pool initialization, builder allocation |
| Discovery | Directory enumeration/filtering, metadata queries, path handling, inventory and identity insertion |
| Source I/O | Opens, read bytes/calls, framing and record boundary work; elapsed read wait where measurable |
| Source fingerprints | Hashing performed during reads and final fingerprint assembly |
| Parsing/validation | Format decoding, logical value construction, declared/schema conversion and validation, fallback handling |
| Text preparation | String traversal, decoding, normalization, case processing, custom text transformations |
| Bigrams | Gram generation/deduplication, record insertion, batch finish/compression, final index assembly |
| Structured postings | Field selection/traversal, typed scalar/structural postings, custom mappings, statistics |
| Inference | Observation/admission and finalization, only when explicitly enabled |
| Sorting | Key extraction/collation and final sort/index construction |
| Relations | Per-record contribution extraction and collection-wide resolution/validation |
| Coordinator | Receive wait, ordered admission/merge, slot remapping, buffering/backpressure |
| Final checks/setup | Inventory/diagnostic/index validation, identity lookup/order, status refresh, pool teardown |
| Persistence | Serialization/compression, write/flush under the existing contract; outside construction total |

Do not silently remove existing fingerprints, validation, metadata checks, or default structural indexes while measuring them.

Record quantities that make timings interpretable: files, records, logical/fallback records, source bytes, decoded text bytes where available, indexed fields, selected sort fields, bigram stores/keys, postings, relation edges, and batch counts.

For ordered admission, distinguish an idle coordinator waiting for workers from actual work on the serial critical path. Identify whether later completed batches wait behind an earlier slow batch and whether memory admission limits leave workers idle.

A high worker-duration total alone is not an optimization priority; determine whether the work limits elapsed completion.

## 5. Profiling and memory

Use an available sampling profiler for a separate release-build construction run with symbols. Include CPU attribution to parsing, hashing, normalization, allocation/copying, bigram operations, bitmap/posting merges, collation, and finalization.

Use system-level observations where supported to distinguish CPU work from I/O/blocking: task CPU time, utilization, context switches, page faults, and relevant storage activity. Do not interpret a function's on-CPU sample share as its share of wall time.

Measure memory in a fresh process before query suites or other alphabets:

- After discovery.
- After extraction/admission.
- After finalization and temporary-builder release.
- After snapshot writing.
- After repository release, where useful to distinguish retained allocations from live data.

Separate RSS/peak RSS from live payload/capacity estimates and allocator-retained memory. Include inventory/paths/record IDs, index structures, relations, temporary batches, worker buffers, and caches. Do not count shared Arc-owned data multiple times.

Use available allocation profiling in a separate run if needed; measure allocations/copies per record or byte and top allocation sites. Avoid adding a production allocator/dependency solely to complete this report. If unavailable, report the gap and preserve the measurements that can be obtained.

Do not compare Unicode RSS after an ASCII build with a fresh predecessor process and attribute the difference to the index.

## 6. Benchmark matrix

Start with the real configured Musenalm workload on unchanged master. Use a build-only benchmark invocation through the existing example, adding a narrow option if necessary, so query execution and snapshot restoration do not contaminate construction timings.

### Required matched conditions

- ASCII and Unicode, in separate fresh processes.
- Production/default build-worker count, recording the actual resolved value.
- Seven repeated warm-cache build trials after warm-up.
- Instrumentation disabled versus enabled, paired or interleaved to estimate overhead.
- Snapshot writing measured separately and consistently enabled/disabled across comparisons.

Capture raw samples, medians, IQR/range, hardware, compiler/profile, exact commit, dirty state, source/schema fingerprints, worker count, cache assumptions, and commands.

Run separate profiled experiments:

1. Build workers 1, 4, and the detected default, deduplicating equal settings. Keep field/index semantics identical.
2. Configured versus explicit inference, keeping each as its own measured workload.
3. Plain unconfigured/no-inference opening using its real existing policy; inventory what it actually builds rather than assuming the staged branch's base-only policy exists on master.
4. One representative large/JSONL fixture to expose framing and batching differences.

Treat these as a bounded diagnostic matrix, not an exhaustive parameter sweep. Begin with the main profile and run extra trials only where necessary to resolve a finding.

A colder-storage experiment is useful if available, but label advisory data-page eviction accurately; it does not reset metadata caches or prove physical cold reads. Keep warm and colder results separate. Do not change global machine settings or disturb unrelated work to manufacture a cold run.

Do not add feature-disable ablations in the first measurement round. If a cost remains ambiguous, propose a narrowly isolated benchmark-only ablation, clearly state its changed semantics, and never report its output as a normal equivalent index.

## 7. Validation and overhead acceptance

Instrumentation must not change indexed records, ordering, validation/fallback classification, stored values, or query answers. Run focused build/index tests and reference query checks on the profiled and unprofiled output. Run formatting, workspace tests, and applicable benchmark checks required by current repository instructions for the final instrumentation change.

Counters should have coherent invariants: processed file/record counts, source bytes, nonnegative exclusive durations, and reconciled top-level elapsed intervals. Do not write brittle tests expecting a fixed timing.

Report paired profiling overhead. If added instrumentation changes median construction by more than approximately 3%, reduce its granularity or separate detailed profiling from the timing run. Do not subtract an assumed overhead and invent corrected stage durations.

The final report must distinguish measured values, sampled estimates, and hypotheses. No optimization is justified solely by a source-code impression or a large allocation count without relating it to elapsed time/memory.

## 8. Deliverable and next decision

Save raw evidence under `bench/results/index-construction-profiling/<run-id>/` using the established manifest format. Write `docs/ai/generated/index_construction_profile.md` with:

1. Exact master baseline and instrumentation commits.
2. End-to-end exclusive timing table, with the residual gap accounted for.
3. Worker/coordinator timeline and parallel-scaling observations.
4. CPU profile and allocation/memory breakdown.
5. Source I/O, metadata, hashing, text, bigram, posting, sorting, and relation costs.
6. Differences across alphabets, worker counts, and explicit inference.
7. Instrumentation overhead and unavailable evidence.
8. The three largest credible opportunities, each with measured cost, likely mechanism, expected scope, and an upper bound on possible elapsed savings.

Do not sum overlapping savings or promise that removing 100 ms of aggregated worker time saves 100 ms of build wall time. Rank by practical impact and confidence. Show whether each proposal reduces total work, serial work, memory pressure, or waiting.

Stop after profiling and the report. The next optimization should be selected from those measurements in a subsequent user decision. Do not treat this task or its report as permission to implement an architectural change.

Move this completed specification and its appended implementation report according to the repository task lifecycle, applying the explicit master authorization for this task. Preserve the unmerged staged-construction branch and integration worktree.


## Implementation report

- Task identifier: `index-construction-profiling`. Completed 12 November 2026.
- Base commit: `b19df1ed4a38c10c879fa3b0c72b1650a0dab203`; authorized integration branch: `master` (explicit direct-on-master exception in this specification). Worktree: `/home/simon/source/objekte`. No subordinate branches or worktrees were created for this task.
- Instrumentation and benchmark commits, each pushed to `origin/master`: `17b8beaaac5b`, `98b070711d2e`, `3044443ef9a2`, and `8f35ff8510f1`. Evidence and analysis: `03beac164084`.
- Added a fresh-process build-only release harness, opt-in sampled construction profiling, a deterministic JSONL and configured-subset fixture generator, and process I/O deltas. No indexing policy, validation, scheduling, query, compression, persistence, or other production optimization was changed. Raw samples, manifests, profiler outputs, system observations and test logs are under `bench/results/index-construction-profiling/2026-11-12-master-b19df1ed-8f35ff85/`. The ranked findings, timer boundaries, reproducibility details and limitations are in `docs/ai/generated/index_construction_profile.md`.
- Validation: `cargo test --workspace --locked` passed, 808 tests passed and 17 ignored; `cargo clippy --workspace --all-targets --locked` passed with existing warnings; 19 focused build tests passed (2 ignored); `git diff --check` passed. The five checked reference queries Q001, Q108, Q116, Q120 and Q194 had identical profiled/unprofiled digests and matching build counters. Python fixture script compilation passed. The repository-wide spacing script still reports unrelated preexisting formatting in files such as `crates/cli/src/execution.rs`; touched Rust files were formatted.
- Main seven-pair warm-cache benchmark at the default 16 workers: ASCII control/profiled medians 2,519.428/2,509.750 ms, paired profiling overhead +0.062%; Unicode 2,672.899/2,691.567 ms, overhead +0.535%. Separate bounded tests measured workers 1/4/16, explicit AutoIndex, plain unconfigured, a 16,384-record JSONL fixture, snapshot persistence, `/proc/self/io`, gprof, Valgrind Callgrind and DHAT. All scenario differences and overlap cautions are in the report. No added profiling regression exceeded the ~3% acceptance threshold.
- Limitations: all full-corpus trials were warm-cache; `samply` could not access perf events at `perf_event_paranoid=2`, so gprof and Valgrind were used. Detailed normal-file timing is sampled, JSONL per-record timing is not; decoded text bytes and worker idle time were not separately counted. Tool profiles on the subset are not scaled to full-corpus wall time. A colder-data experiment was omitted to avoid disrupting unrelated cache consumers. Snapshot writing was measured after build. No feature-disable ablation or production optimization was performed.
- The pre-existing `integration/staged-index-construction` branch and its worktree were preserved without merging, cherry-picking, deleting, or modifying them. The source specification was initially untracked in the original checkout; its complete bytes were copied unchanged before this report was appended. The original untracked copy can be removed only after this completed document is committed and pushed and its prefix rechecked.
