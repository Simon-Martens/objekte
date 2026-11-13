# Objekte: Reprofile index creation and loading

## Objective

Explain where the current implementation spends time creating an index and restoring an existing snapshot. Produce a reproducible, measured ranking of the remaining costs, with code locations and bounded optimization proposals.

This is a profiling task. Reuse existing instrumentation, complete missing attribution, and run measurements. Do not implement performance changes, remove functionality/validation, change the snapshot format, or begin the separately identified snapshot-buffer rewrite.

Answer these questions explicitly:

1. What determines catalogue readiness, the first searchable publication, and full construction completion?
2. How much time goes into discovery, reading/parsing, text normalization and bigrams, field postings, sorting/relations, and segment finalization/publication?
3. Is the remaining construction delay CPU work, I/O, serial work, allocation, or waiting between workers and the publisher?
4. Why does loading a saved index still take a substantial fraction of fresh construction time?
5. Which measured costs are avoidable, what is their realistic wall-time significance, and what memory or complexity tradeoff would a change introduce?

## Starting point and workflow

Master now contains Plans 1–3 and both follow-up repairs. Verified master at task preparation: `028f62b10618db61410689db4ad86e6d4fbff036`. The last cumulative review tip was `a5fdd933255af35974ef81f8d3d522aaa28fb933`.

- Task ID: `index-creation-loading-profiling`.
- Integration branch: `integration/index-creation-loading-profiling`.
- Integration worktree: `/home/simon/source/objekte-integration-index-creation-loading-profiling`.
- Start from current master after fetching and confirming it contains the merged repairs. Record the actual base SHA and relevant intervening changes. Do not silently profile an obsolete pre-repair branch.
- Keep the original checkout on master and unchanged. Preserve unrelated staged/untracked files and task documents. Push after every commit; no force-push or master merge.
- Follow `docs/ai/README.md`. One agent may implement and supervise; optional separable profiling work must use sibling worktrees under `/home/simon/source`. Run performance trials sequentially on the measurement host, without competing builds or benchmark agents.

## Existing instrumentation: retain and extend

The following code is present at the verified master:

| Component | Existing instrumentation |
|---|---|
| `crates/core/examples/bench_index_construction.rs` | `--profile`, `--incremental`, `--restore-snapshot`, `--snapshot`, worker/alphabet selection, result manifests, RSS/I/O measurements, query checks |
| `crates/core/src/repository.rs` | `StartupStats`, `BuildProfileStats`, detailed direct-build discovery/finalization/component attribution |
| `crates/core/src/build.rs` | `BuildWorkProfile`, sampled worker/component timings, scheduler/admission/ready-queue statistics |
| `crates/core/src/repository_incremental.rs` | Current incremental construction, publication/finalization intervals, scheduler totals |
| `crates/core/src/storage.rs` | Snapshot save-section timings, optional byte attribution, section reader/decoder |
| `crates/core/src/lifecycle_open.rs` | Lifecycle restore, lookup reconstruction/publication, automatic background reconciliation |

Two confirmed gaps need attention:

1. The direct benchmark emits the detailed `build_profile`, but the incremental benchmark principally emits readiness, total build, memory, and counts. The incremental final startup state also does not assemble the full detailed profile. Passing `--profile` alone therefore does not provide complete attribution for the production incremental path.
2. `run_restore` measures the outer load call, but does not attribute its internal phases. `read_section` currently reads a section to verify its checksum, seeks back, and reads it again for deserialization. Lifecycle payloads then undergo segment decoding and structural checks. Measure these costs rather than assuming that one file read explains loading time.

Trace the actual desktop and CLI opening paths before selecting benchmark calls. Use incremental construction and lifecycle snapshot restoration as the primary workloads. The older direct repository builder is a diagnostic comparison only.

## Phase 1: Establish an honest baseline

Build the release benchmark once, outside timed runs, using the repository's locked dependencies. Freeze the code revision, corpus, schema, alphabet, inference policy, worker count, and snapshot format for the campaign.

Primary corpus: the established Musenalm corpus at `/home/simon/source/objekte/examples/musenalm`, configured extended definitions, inference disabled, and the normal resolved worker count. Prior measurements used 148,486 files, ASCII bigrams and 32 workers; verify these rather than assuming the host or corpus is unchanged.

Record CPU, affinity, worker resolution, compiler/build settings, filesystem/storage, corpus/schema fingerprints, snapshot bytes/version, cache conditions, and relevant background activity. Keep manifests and exact commands beside raw samples.

Measure independently:

- **Fresh construction:** no snapshot restore or write in the timed interval. Record public call start, catalogue availability, first indexed publication, observed first result, full completion, and caller return. Preserve the existing first-result probe and account for its work separately from construction.
- **Fresh-process restoration:** load a compatible completed snapshot created once for this revision/configuration outside the restore timing campaign. Verify that restoration actually occurred; silently rebuilding an incompatible cache invalidates the sample.
- **Reconciliation after restore:** report its start/completion and work separately from the index-ready return time. It may overlap restoration/foreground activity; it is not an additive tail unless measurements establish that ordering.

Reference figures from the previous cumulative repair campaign were approximately 963 ms construction and 758 ms restore, with a 132.71 MB snapshot. They are context, not replacements for the new controlled baseline.

## Phase 2: Complete construction attribution

Reuse existing profile structs, sampled timers, and aggregation wherever possible. Carry worker and finalization observations through the incremental path and export them from the benchmark. Do not add a timer/atomic allocation for every character, gram, or posting.

Provide three separate views:

1. **Elapsed timeline:** exclusive outer phases and timestamps that reconcile with the measured public operation. Show nested/overlapping work explicitly.
2. **Accumulated worker work:** component totals, concurrency and sample coverage. These may exceed elapsed time and must not be presented as additive wall-time percentages.
3. **Serial delays and dependency chain:** publication/admission work, queue waits, slow batches and the longest chain delaying readiness or completion.

Required attribution, using appropriate nested timers:

| Area | Distinguish |
|---|---|
| Discovery | Directory enumeration, metadata collection, path/ID inventory construction, ordering/insertion |
| Setup | Thread-pool initialization, collection/schema setup, work scheduling |
| Source processing | Opens/reads, bytes, framing, parsing/validation, logical extraction; separate nested extraction callbacks |
| Text indexes | Normalization, ordinary/skip bigram generation, packing/compression, batch merge/finalization where hooks permit |
| Field indexes | Existing scalar/length/presence and structural postings; inference observation/admission when enabled |
| Other indexes | Sorting, identity ordering, relations and custom index work |
| Segment publication | Sealing/index finalization versus publication/catalogue bookkeeping and callbacks |
| Completion | Fingerprint finishing, inference summary, final consistency checks, final publication and teardown |
| Waiting | Worker execution, ready-queue residence, coordinator receive wait, admission/backpressure, active-worker counts |

The existing publication interval includes sealing/indexing; do not report all of it as lock/publication overhead. Wait intervals can overlap worker execution and must not be subtracted or summed arbitrarily.

Report per-collection and per-segment contributions for the dominant costs, including bytes/records and batch sizes. Preserve sampled denominators: sampled files, records, bytes, callback time and sampling policy. Do not multiply sampled durations by a constant and describe the result as measured total work without checking representativeness and labelling the estimate.

## Phase 3: Attribute loading

Add opt-in load profiling with the smallest practical internal interface. Keep it out of the persisted format and leave normal loading behavior unchanged. Export the profile through the existing benchmark rather than creating a second loader.

Measure the following independently or as clearly labelled nested intervals:

1. File open, header/version/configuration checks, and outer section framing.
2. Checksum traversal, including bytes supplied to hashing, read calls, hashing work and seeks.
3. Outer deserialization: base inventory/collections and lifecycle envelope/encoded segment buffers.
4. Supplemental metadata decoding, reference resolution, catalogue/path-map reconstruction, and file-version/liveness restoration.
5. Per-segment decoding and index-family reconstruction; separate bigrams, postings, sorting and relations where sampling or existing boundaries permit.
6. Structural validation of loaded metadata/indexes, including domain, identity, sort, relation and checksum checks. Count repeated traversals of the same data across layers before suggesting they can be combined.
7. Reconstruction of derived `file_index`/`id_lookup`, cross-segment uniqueness checks, base partitioning if invoked, and publication setup.
8. Background reconciliation: directory/stat work, changed-file detection, content reads/parses if any, and whether it delays the first usable query.

Separate logical bytes processed, filesystem read requests, and physical storage I/O. A warm-cache reread may consume CPU/memory bandwidth without another disk read. Record the checksum pass and deserialization pass explicitly; do not assume twice the logical bytes means twice the physical I/O.

Do not infer zero source activity from zero `build_source_reads` alone: those are construction counters. Verify actual restore/reconciliation work with correctly scoped counters or tracing. Loading snapshot bytes is not source-document parsing. Validation of a persisted index is also distinct from revalidating source documents on search.

Record loaded index readiness and reconciliation completion separately. Keep automatic reconciliation enabled for the normal baseline. If an isolated decode-only diagnostic is necessary, label it as a diagnostic, preserve the real-path measurement, and do not present it as user-visible startup time.

## Timing and memory methodology

- Run at least seven fresh-process repetitions of each primary construction and restoration workload with profiling disabled, and seven with profiling enabled. Alternate order and report median, quartiles and raw values. Quantify profiling overhead; keep uninstrumented times as the performance baseline.
- Prefer low-overhead counters and coarse timers; use sampling for hot work. Investigate profiling overhead above roughly 5% and reduce or isolate instrumentation if it materially changes scheduling.
- Use warm filesystem cache as the principal comparison to previous reports. A fresh process is not a cold-cache run. Cold-cache measurements are optional and must use a documented isolated procedure; do not globally drop host caches or disturb unrelated processes.
- Supplement with a small worker-scaling diagnostic: one worker, an intermediate count, and the normal count, three runs each. Adjust counts to the machine. Use this to distinguish parallel work from serial limits, not to create a large Cartesian benchmark matrix.
- Add a small Unicode cross-check. Add unconfigured/no-inference and explicit-inference diagnostic configurations when needed to attribute optional indexing costs, using the same corpus or a clearly identified representative fixture. Keep those results separate from configured baseline comparisons.
- Use the host's existing CPU sampling tools, such as `perf`, to corroborate dominant code paths and distinguish processing from wait/I/O. Keep samples/flamegraphs and symbolization settings. If sampling is unavailable, report that limitation and continue with supported timers/counters; do not invent sampled stacks.
- Record process peak RSS and post-build/post-load RSS, allocation/retained-byte evidence when available, and encoded/decoded segment-buffer overlap. RSS samples and nonexclusive heap estimates are not interchangeable. A process high-water mark cannot be reset between phases.
- Creating the restore fixture may require a snapshot save. Keep its time out of build/load totals; label it separately. The existing approximately 104 MB retained encoding buffers are a hypothesis to investigate, not a reason to make this task a save-path rewrite.

Aim to explain at least 95% of each outer elapsed interval through correctly nested measured phases. Investigate material unexplained time rather than hiding it in a broad miscellaneous category. If attribution remains incomplete, show the residual and what additional evidence would resolve it. Do not add overlapping worker sums to manufacture reconciliation.

## Existing command starting points

Verify flags against the selected revision. The following interfaces exist at the task-preparation revision; repeat invocations with distinct result directories and add the new exported attribution as needed:

~~~bash
cargo build --release --locked -p objekte-search-core --example bench_index_construction

target/release/examples/bench_index_construction \
  --root /home/simon/source/objekte/examples/musenalm \
  --incremental --alphabet ascii --workers 32 --profile --check-queries \
  --result-dir bench/results/index-creation-loading-profiling/construction-profile-01

# Prepare a compatible snapshot outside timed restoration trials.
target/release/examples/bench_index_construction \
  --root /home/simon/source/objekte/examples/musenalm \
  --incremental --alphabet ascii --workers 32 --snapshot --check-queries \
  --result-dir bench/results/index-creation-loading-profiling/restore-fixture

target/release/examples/bench_index_construction \
  --root /home/simon/source/objekte/examples/musenalm \
  --alphabet ascii --workers 32 --profile \
  --restore-snapshot bench/results/index-creation-loading-profiling/restore-fixture/index.snapshot \
  --result-dir bench/results/index-creation-loading-profiling/restore-profile-01
~~~

Run these from the task worktree. Use the resolved normal worker count if it differs from 32. Disable expensive snapshot-byte diagnostics during timing runs. Perform correctness checks after the relevant timed interval and distinguish their memory/I/O from the startup measurements. Preserve baseline/profile pairing and snapshot identity.

## Validation and boundaries

Instrumentation must not change results, publication order/semantics, index contents, snapshots, or inference policy. It must not introduce source rereads, query-time freshness checks, or additional parsing. Profile-disabled execution must retain its normal low overhead.

Add focused regression coverage for populated incremental/load profiles, phase identities/counts, and profile-on/off result equivalence. Do not write fragile unit assertions about exact durations. Preserve corrupt-cache rejection, validation, and cancellation behavior.

Run appropriate core/benchmark checks and the workspace suite if shared startup/loader interfaces change. Use the existing query digest checks to confirm equivalent construction and restoration. Broaden query campaigns if a behavioral change or unexplained mismatch appears; this task does not require repeatedly running unrelated UI tests. Record inherited lint/formatting failures without unrelated cleanup.

## Required report and handoff

Write `docs/ai/generated/index_creation_loading_profile_results.md` with:

- A concise answer to what currently dominates creation and loading, and the absolute time available to improve.
- Separate tables for unprofiled wall time, exclusive phase time, accumulated/sampled worker work, and per-collection/per-segment hotspots. Include an elapsed timeline or flamegraph where useful.
- Readiness and reconciliation boundaries; CPU/I/O/wait evidence; profiling overhead; source/snapshot read counts; and peak/retained memory.
- A ranked list of at most five next optimization candidates. For each, cite functions/lines, measured evidence, the affected wall-time interval, a defensible upper bound or range for potential savings, and memory/correctness/complexity implications. Mark hypotheses explicitly. Do not convert CPU samples directly into promised wall-time savings.
- A distinction between required work and potentially redundant work. In particular, explain checksum traversal, structural validation, reconstructed lookups and buffer lifetimes rather than recommending deletion without a correctness argument.
- Environment, complete commands, revisions, raw measurements, limitations, and comparisons to previous results only where protocols match.

Store raw samples/manifests and reproducible scripts under `bench/results/index-creation-loading-profiling/`. Keep large generated snapshots local rather than committing them; record format, hash and reproduction command. Retain reasonably sized profiling artifacts or document how to regenerate them.

Commit the completed specification under `docs/ai/done/objekte_index_creation_and_loading_profiling_specification.md` with its implementation report. Deliver the pushed integration tip, report links, checks performed, and the measured next priorities. Do not optimize the identified bottlenecks or merge into master as part of this task.


## Implementation report

- Task: `index-creation-loading-profiling`; completed 2026-11-13. Base master: `8682dcb201d3cd0b6c1c0bf0b1a5c552f1710b67`, including the intervening TUI completion commit after the original task-preparation SHA. Integration branch: `integration/index-creation-loading-profiling`; integration worktree: `/home/simon/source/objekte-integration-index-creation-loading-profiling`.
- Implementation: added opt-in incremental worker, collection/admission, segment-seal and publication attribution; added opt-in checked snapshot section, loader and segment decode/validation profiling; exposed startup statistics to the benchmark; exported load/restore, readiness, memory and process-I/O data. No indexing optimization, validation removal, snapshot format change, or master merge.
- Contributors: one supervising/implementing agent; no subordinate branches or worktrees. Implementation commits: `e00e364f8a` (core and benchmark profiling), `f131fa6149` (local campaign artifacts ignored by default), `8d4cfdc4c7` (per-collection admission attribution). All were pushed immediately.
- Validation: `cargo check --locked -p objekte-search-core --example bench_index_construction --quiet`; focused incremental/load profile-on/off regression test; `cargo test --locked --workspace --all-targets --quiet` (passed, no failures); release benchmark build; 28 final primary trials with matching five query digests and 14/14 actual restores. `git diff --check` passed.
- Results: the complete methodology, raw samples, commands, environment, limitations and ranked proposals are in [Index creation and snapshot restoration profile](../generated/index_creation_loading_profile_results.md). Unprofiled medians: 1,015 ms fresh construction and 802 ms snapshot restore; profiled outer residual medians 3.7 ms and 2.2 ms respectively. The prior ~963/~758 ms values came from a different revision/campaign and are context, not a matched regression.
- Limitations: warm-cache measurements on a shared host; CPU stack sampling blocked by `perf_event_paranoid=2`; one run each for plain and explicit inference diagnostics. Snapshot fixture is retained locally but excluded from Git due its 132.71 MB size; its hash and reproduction command are in the report. No out-of-scope issue was implemented.
- Cleanup: the original checkout remains on master; unrelated user files and index state are preserved. The integration worktree and branch remain for review/merge. Temporary task memory is removed after this report commit and push.
