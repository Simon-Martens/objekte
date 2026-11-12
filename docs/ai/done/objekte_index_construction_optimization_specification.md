# Objekte: parallel metadata preparation and index-construction experiments

Task identifier: `index-construction-optimization`  
Prepared: 12 November 2026  
Inspected baseline: `737cfb7745ff15915804553ab583af260ac0f2e4` (`master`)  
Delivery branch: `integration/index-construction-optimization`

## 1. Objective and scope

Implement bounded parallel metadata preparation during fresh index construction. Independently implement and measure four allocation experiments on four separate branches. Investigate batch admission with a component breakdown, then test one narrowly justified admission optimization on its own branch.

The priority is lower elapsed construction time without materially increasing peak memory, retained memory or index size. Allocation reductions are supporting evidence; they are not substitutes for elapsed-time measurements. Preserve query results, record identities, source validation, indexing selection and snapshot compatibility.

This document requests implementation work when the user assigns it to an agent. It does not request another design-only report. A completed experiment may legitimately conclude that its change should not be integrated.

The work consists of:

| Track | Required result |
| --- | --- |
| M: Metadata | Implement parallel metadata/path preparation; preserve deterministic catalogue insertion; measure the result |
| E1: Input buffers | Test reuse of file-reading buffers |
| E2: Bigram scratch | Test reuse of bigram accumulator scratch across batches |
| E3: Field selection | Test removal of temporary field-selection allocations in construction |
| E4: Logical deserialization | Test direct JSON deserialization into Objekte logical values |
| A0/A1: Admission | Measure coordinator operations and scheduling delays; test one bounded optimization supported by the findings |
| Integration | Combine only justified changes; rerun representative correctness and performance checks; report rejected experiments too |

Explicit exclusions:

- No `io_uring`, Compio, memory-mapped input, new asynchronous I/O framework or new runtime dependency.
- No staged/two-pass indexing. Leave `integration/staged-index-construction` unmerged.
- No changes to configured field coverage, automatic-inference defaults, text semantics or query semantics.
- No general finalization rewrite. Parallel relation resolution/finalization is deferred to a later task.
- No removal of validation, hashing or existing error handling to manufacture a speedup. Do not add additional source-freshness checks either.
- No larger outstanding-job or scheduling-memory limits as an optimization.
- No public API, CLI flag or desktop interface expansion unless an existing entry point demonstrably requires a small compatibility repair.
- Do not merge into or update `master` without a separate explicit user command.

## 2. Evidence and implementation starting points

Read these repository files before editing:

- `AGENTS.md` and `docs/ai/README.md`.
- `docs/ai/generated/index_construction_profile.md`.
- `bench/results/index-construction-profiling/2026-11-12-master-b19df1ed-8f35ff85/README.md` and the referenced benchmark invocation/manifests.
- `crates/core/src/repository.rs`, `build.rs`, `source.rs`, `bigram.rs`, `inventory.rs`, `discovery.rs`, `model.rs`, `reference.rs`, and relevant tests.

At the inspected revision, the configured Musenalm benchmark contains 148,486 records and 193,297,230 source bytes. The reported unprofiled warm medians are 2.519 s ASCII and 2.673 s Unicode on an AMD Ryzen 7 5700G with 16 resolved build workers. These are historical reference measurements, not a substitute for measuring the actual task baseline.

The representative profiled ASCII run has approximately:

| Measurement | Time | Interpretation |
| --- | ---: | --- |
| Discovery | 647 ms | Exclusive interval before pool creation |
| Enumeration component, component median | 98 ms | Directory traversal, filtering and path ordering |
| Catalogue component, component median | 554 ms | Metadata, path handling and inventory insertion combined |
| Source-work interval, component median | 1,293 ms | Parallel worker work overlapping coordinator work |
| Coordinator admission, component median | 1,149 ms | Active processing, not idle waiting |
| Receive/wait, component median | 144 ms | Receive calls, including blocking and handling overhead |
| Maximum recorded ready-batch wait | 1,283 ms | Overlapping time retained by one result; not recoverable wall time |

Do not add component medians to reconstruct a particular trial or add overlapping worker/coordinator durations.

The existing allocation evidence is a separate configured 5,338-record DHAT run: 296.87 MB allocated in 1,387,369 blocks, about 260 allocations per record including setup and teardown. Important stacks include JSON/logical container construction and file-buffer growth. This is not a full-corpus allocation count and must not be extrapolated into promised savings.

Confirmed code behavior at the inspected revision:

1. `Repository::build_fresh` calls `discover`, iterates sorted paths, calls `std::fs::metadata`, and inserts inventory entries serially. It creates the build pool afterward.
2. Normal source extraction creates a fresh `Vec<u8>` and calls `read_to_end` for every file. Record ranges borrow slices from that buffer.
3. Ordinary JSON construction parses into `serde_json::Value`, then consumes it into the logical `Value`. String/key ownership moves; array/object containers are rebuilt. This is not a second parse of the source bytes.
4. Ordinary JSON build extraction does not collect owned text segments. Bigram construction borrows logical strings and streams folded characters without materializing a full folded string.
5. `JsonDecodedScalars` has different semantics and currently retains both scalar text and logical values. Do not silently replace it with ordinary JSON semantics.
6. Bigram accumulator scratch is reused within a batch but recreated for the next batch. Completed contribution storage is different from reusable scratch.
7. Some construction paths call `data_values`, converting the existing inline-capable selection into a heap `Vec`.
8. Batch dispatch and ordered admission share one coordinator. Outstanding limits include completed but unadmitted results. The scheduler also precomputes first jobs from up to two later collections; their long waits can be intentional.

## 3. Branch, worktree and supervision requirements

Follow the repository workflow. This is a new integration task; the earlier permission to profile directly on `master` does not authorize implementation commits there.

- Original checkout remains on `master`, with user changes preserved.
- Integration worktree: `/home/simon/source/objekte-integration-index-construction-optimization`.
- All subordinate worktrees are siblings under `/home/simon/source`, never under `/tmp` or inside another checkout.
- One supervisor owns the integration worktree. Delegate independent branches where useful; each implementer owns only its assigned worktree.
- Push immediately after each commit, including integration and report commits. Confirm success before further commits on that branch.
- Never force-push or rewrite published experiment history.
- Serialize performance measurements on the benchmark host. Other agents must stop builds, tests and benchmarks there during a reserved measurement interval.

Record the actual starting `master` commit. If it has advanced beyond the inspected revision, inspect relevant changes and adapt to the current implementation without restoring older code.

Create the integration branch from that commit. If a small shared benchmark-script change is necessary, commit it before experimentation and verify that it does not change construction behavior. Record the resulting common baseline as `B`.

Use the following branch layout. All branches except A1 start at exactly `B`; do not stack the four allocation experiments on one another or on the metadata change.

| Track | Branch |
| --- | --- |
| M | `task/index-construction-optimization/metadata-parallelism` |
| E1 | `task/index-construction-optimization/file-buffer-reuse` |
| E2 | `task/index-construction-optimization/bigram-scratch-reuse` |
| E3 | `task/index-construction-optimization/field-selection-allocation` |
| E4 | `task/index-construction-optimization/direct-logical-deserialization` |
| A0 | `task/index-construction-optimization/batch-admission-analysis` |
| A1 | `task/index-construction-optimization/batch-admission-optimization` |

A1 starts from the recorded A0 measurement commit and compares against that same commit. Keep any diagnostic instrumentation equivalent between the A1 control and candidate. Check unprofiled behavior as well.

Subordinate worktree names follow `objekte-task-index-construction-optimization-<subtask-id>`. Record full paths and commits in the report. Keep experimental branches available for review, including rejected experiments. Follow the repository's cleanup rules; do not force-remove worktrees containing unmerged experimental work.

## 4. Track M: implement parallel metadata preparation

### 4.1 Design

Keep existing discovery selection and ordering. Parallelize the per-file preparation that currently follows discovery.

1. Resolve the normal build-worker policy and create the existing Rayon build pool before metadata preparation. Reuse this pool for later source construction. Do not create nested pools or additional independent worker budgets.
2. Keep collection order, `discover` filtering, lexical ordering, symlink handling and alias selection unchanged.
3. Process the selected paths in bounded windows or chunks. For each path, independently prepare the metadata, repository-relative path, `SourceRevision` and creation time currently needed by inventory insertion.
4. Collect preparation results in input order. An indexed parallel iterator is suitable; preserve the ordinal explicitly if another mechanism is used.
5. Insert prepared entries into `FileInventory` serially in the existing order. This keeps first-seen `FileId` assignment and overlapping-collection membership behavior stable.
6. Release each preparation window before retaining another large window. Existing discovered paths may still be retained; avoid adding another corpus-sized set of copied paths and metadata.

Use ownership moves for prepared paths. Do not clone the full discovered path vector to feed workers. Do not protect the existing `FileInventory::add` with a per-file mutex and call that from all workers: it would preserve much of the same serialized work plus lock overhead.

Prefer conservative fixed-size chunks tied to the resolved worker count; choose and document the bound from a short experiment. One worker must remain a valid, inexpensive sequential case. Test 1, 4 and the normal detected worker count, without treating all logical CPUs as an obligation to maximize concurrent syscalls.

The pool's earlier lifetime must be included in total build timing and handled on errors. Keep phase counters meaningful: report pool creation separately, rather than making discovery appear faster simply by moving work outside its timer.

### 4.2 Correctness constraints

- Preserve selected files, path ordering, record/file identities, collection membership and source revision fields on an unchanged corpus.
- Preserve skip/follow-within-root symlink behavior, alias deduplication and root containment checks. The parallel stage consumes already-selected paths; it does not weaken discovery rules.
- Preserve the same required metadata fields and error semantics. When several paths fail, return the failure for the earliest applicable original ordinal; do not expose a scheduling-dependent error solely because a parallel `Result` collection short-circuits differently.
- Preserve overlapping-collection revision consistency checks. Do not replace them with a global metadata cache in this experiment.
- Preserve behavior for empty collections, non-UTF-8 paths, unavailable creation timestamps and permission/I/O errors.
- Do not add repeated `stat` calls, reread sources or introduce stronger guarantees for files changing during a fresh build. Keep the existing contract and obtain only the metadata already required.

### 4.3 Measurements and acceptance

Separate elapsed times for enumeration, parallel preparation and ordered insertion. Record total discovery and total build as well. Summed worker durations are optional and must be labeled separately from elapsed time.

The change must actually run independent preparations concurrently when multiple workers are available; verify this with a deterministic concurrency test or diagnostic counter, not a flaky elapsed-time unit test. Compare catalogue contents against one-worker execution.

Measure the metadata branch against `B` using section 10. Report where the remaining discovery time resides. If parallel preparation produces no useful total improvement, retain the branch and evidence, explain the result, and do not describe it as a proven performance fix.

## 5. Experiment E1: reusable file input buffers

### Hypothesis

Fresh allocation and capacity growth of a file buffer for every normal file creates avoidable allocator traffic. The existing discovery size can help reserve storage without an additional syscall.

### Implementation

- Add a construction-internal extraction path accepting reusable input storage. Keep the existing public reader interfaces compatible where possible.
- First reuse one buffer across files within a batch. This alone replaces up to 512 separate buffer lifetimes with one. Reuse across batches through worker-owned storage is optional if it remains small and independently measurable; report the actual lifetime implemented.
- Clear length between files while retaining reasonable capacity. Reserve using the already available discovered size as a hint, with checked conversions and a bounded reserve policy.
- Continue reading to EOF according to the existing contract. Do not truncate to the discovered size or assume a metadata hint is an infallible current byte length.
- Hash and parse the bytes already read. Preserve digests, record boundaries and malformed/unreadable handling.
- Account for paths that transfer ownership of malformed bytes: move or replace storage safely, then recover/reinitialize scratch as necessary. Do not introduce dangling references or clone every successful record to simplify the exceptional case.
- Nothing retained in a completed batch may borrow from the reusable file buffer. Return/reset storage only after all synchronous consumers finish.
- Bound retained capacity after unusually large files. Document the bound and test a large-file-followed-by-small-files case.
- Leave the separate bounded JSONL/framed-source machinery unchanged unless the shared internal change requires a compatibility adjustment.

### Evidence

Record input-buffer growth/allocation events and retained capacities in an opt-in diagnostic run, plus ordinary build time and fresh-process peak RSS. Compare bytes read, digests and query results. Do not introduce a new allocator just to count allocations.

## 6. Experiment E2: reusable bigram accumulator scratch

### Hypothesis

The accumulator already reuses memory within each batch, but recreates scratch for subsequent batches. Retaining bounded scratch per worker may reduce repeated allocation and initialization.

### Implementation

- Separate reusable accumulator storage from the owned `BigramContribution` returned to the coordinator.
- Reuse the ASCII normal/skip word arrays and touched-index vectors across batches; reset every touched word and all record/chunk bookkeeping correctly.
- Reuse Unicode accumulator map capacity under a documented retention bound. Preserve or improve the existing oversized-map trimming policy without creating repeated shrink/grow churn on ordinary input.
- Retain laziness: unused text stores must not allocate their full ASCII tables just because a worker exists.
- Scope reuse to an actual worker/task storage lifetime with exclusive ownership. Rayon jobs can execute on different workers; do not assume a batch ordinal identifies a worker.
- Handle different collections, text-store counts, schemas and alphabets without leaking bits, record offsets or field associations between batches.
- Do not retain an entire finished batch builder or its posting/record output merely to preserve accumulator tables.
- Do not copy completed contributions back into scratch or recycle their storage before the coordinator releases ownership.
- Keep the number of retained scratch instances bounded by the worker/storage policy, not by the number of completed batches or discovered field configurations.

This experiment does not change gram definitions, mandatory case folding, final Roaring representation or finalization scheduling. The normal insertion path already streams folded characters; do not add a full normalized-text buffer.

### Evidence

Count scratch creation/growth and maximum retained capacity. Test multiple batches, mixed collection configurations, empty stores, malformed records, final partial chunks and both alphabets. Compare final index/query equivalence and peak RSS. Lower allocation traffic with substantially higher retained worker memory is not an automatic acceptance.

## 7. Experiment E3: remove transient field-selection allocations

### Hypothesis

Construction allocates temporary vectors to select fields even when a field has a single value. Existing inline selection or borrowed traversal can avoid these allocations.

### Implementation

- Inspect construction calls to `data_values`, especially configured custom-text and custom-posting paths in `BatchBuilder::logical`.
- Use the existing `data_values_small` where its semantics fit, or a narrowly scoped borrowed visitor/iterator. Avoid converting the inline result straight back into a `Vec`.
- Preserve missing versus present-empty fields, array traversal, occurrence order and multiplicity. Do not replace a multi-value field with its first element.
- Keep `string_values` borrowing strings; do not replace its iterator with a newly allocated collection.
- Inspect intermediate mapped-value/reference vectors in custom postings. Remove only those that can be eliminated without changing mapping failure, completeness or value ownership semantics.
- Keep changes restricted to construction. A general query evaluator rewrite or transformation-trait redesign is outside this experiment.
- `map_text` returns an owned string today. Do not claim to eliminate that separate allocation merely by removing the selection vector.

### Evidence

Use the configured corpus and a focused fixture with single values, nested paths, arrays, missing values and operation mappings. Record selected-value allocation events or a comparable scoped allocation measurement. Report how often the changed path is exercised; a fixture-only improvement is not a measured full-build improvement.

## 8. Experiment E4: direct JSON-to-logical deserialization

### Hypothesis

Ordinary JSON construction allocates serde containers and then reconstructs logical containers. Deserializing directly into the existing logical model can remove intermediate container work without changing the stored model.

### Implementation

- Implement a construction-specific deserialization path using the existing parser/dependencies, preferably a serde visitor/seed rather than a new handwritten JSON lexer.
- Produce the existing Objekte `Value`/`ArrayValue` representation directly. Preserve the same schema coercion and validation stage afterward.
- Preserve exact number semantics, including integer boundaries, decimals/exponents and current rejection cases. Do not route all numbers through `f64`; determine how the baseline preserves number tokens and reproduce that contract.
- Preserve duplicate-key behavior, Unicode and escape decoding, nesting/resource limits, top-level scalar/array/object acceptance, and logical array type/nullability metadata.
- Preserve syntax-error and malformed-source fallback behavior, including usable diagnostic coordinates/categories. If equivalent diagnostics require invoking the old parser on an error path, that is acceptable; do not parse valid documents twice.
- Keep ordinary JSON/JSONL semantics distinct from `JsonDecodedScalars`. Do not lose lexical scalar occurrences, overwritten object members or searchable number text by redirecting the latter reader into the new ordinary path.
- Keep public `Value::from_json` behavior available for other callers. No lifetime-bearing public-value redesign, arena allocator or new dependency is required.
- Do not replace a new container with a subsequent full `Value` clone; verify that the intermediate representation is actually removed.

### Evidence

Compare old and candidate readers directly over a compact regression corpus: escaped/unescaped text, Unicode, empty values, mixed/nested arrays, duplicate object keys, number boundaries, malformed syntax and nesting limits. Then compare normal construction, fallback status/diagnostics and queries on the real configured corpus.

Record allocated bytes/blocks using the existing small allocation fixture when available. Its counts must be reported separately from full-corpus wall time. This is the largest experiment; correctness or maintenance problems are sufficient reason to reject it even if an isolated parser microbenchmark improves.

## 9. Batch admission: measure active work, then test one improvement

### 9.1 Why this is included

The coordinator's roughly 1.15 s admission duration is mainly active processing. A completed batch waiting in memory does not mean its former worker is blocked, nor that the elapsed build could be shortened by that entire wait.

The scheduler creates real possible restrictions: ordered slot assignment, serial merging, a bounded outstanding-job/memory window, and dispatch performed by the same coordinator that merges results. They must be distinguished before choosing a fix.

### 9.2 A0: component breakdown

Add opt-in batch-level timing, avoiding per-record/per-gram timers in normal builds. Keep expensive tracing disabled by default. Account for these components independently:

| Component | Included work |
| --- | --- |
| Callback bookkeeping | Fingerprint binding, build counters and other work outside `CollectionAdmission::admit` |
| Record/inventory admission | File inclusion, diagnostics, record registration, slot checks and identity handling |
| Structured postings | `PostingCollection::merge_shifted` |
| Inference | Optional inference merging, labeled disabled when absent |
| Relations | Relation contribution remapping/merging |
| Custom postings | Configured operation posting merges |
| Bigrams | Raw, fallback and custom gram contribution remapping/merging |
| Structural facts | Logical-record and empty-array bitmap merging |
| Sorting | Sort-key append/remapping |
| Remainder | Destruction, bookkeeping and timer boundaries not captured above |

Reconcile each timed batch against total admission and aggregate by collection. Include useful work counts such as records, gram blocks or posting contributions where they are already cheaply available; do not traverse complete indexes solely to count them in the timed path.

Distinguish these scheduling intervals in a bounded diagnostic trace or equivalent aggregate counters:

- Dispatch to worker start: queued work, not execution.
- Worker start to completion: job elapsed time, not necessarily CPU time.
- Completion to receipt by coordinator: completed result/channel delay.
- Receipt to admission start: result waiting for order and/or coordinator service.
- Admission start to end: actual merge work.
- Coordinator receive-call duration: do not relabel all of it as pure blocking.
- Outstanding-job and memory-budget limits preventing further dispatch.
- Periods with unsubmitted work and unused worker capacity while the coordinator is admitting, where observable from task events.

Timestamp completion in the worker before sending the result. The existing ready-wait measurement begins when the coordinator receives an out-of-order result; it does not include all time already spent in the channel. Explain any remaining measurement limitations instead of claiming a complete CPU-idle breakdown.

When distinguishing order delay from coordinator delay is difficult, report the combined interval honestly. Do not present guessed subdivisions as measurements. Identify prefetched later-collection jobs so intentional early completion is not mistaken for a slow preceding file.

Use one or a few representative diagnostic runs plus unprofiled controls. Confirm that disabled diagnostics do not materially regress the normal build. This is a small construction measurement extension, not a general tracing subsystem.

### 9.3 A1: implement one evidence-led optimization

After A0, select and implement one bounded candidate on the separate A1 branch. Record the measured reason for choosing it before implementation. Suitable choices include:

1. **Dispatch replenishment:** if workers lack submitted work while the coordinator drains ready results, test replenishing available job slots between admissions. Preserve byte/job bounds, ordered admission, error draining and deadlock freedom.
2. **Dominant merge operation:** if one component dominates, reduce its allocation, copying or repeated traversal, or partition independent index work where ownership permits. Measure the operation and whole build.
3. **Independent collection work:** if global ordering unnecessarily delays independently owned collections, test a narrowly scoped scheduling change that preserves each collection's slot order and deterministic global diagnostics/identities. Prove the independence before relying on it.

Do not implement all three as a combined rewrite. Do not simply admit whichever batch finishes first: variable record counts, generated identities, per-collection ordering and diagnostics require an explicit correctness argument. A cheap ordered assignment of final slot ranges may be separated from heavy merging, but only if the selected experiment remains bounded and measured.

Keep task limits and source batch size unchanged so an improvement cannot be attributed to increased temporary memory. Do not reduce validation or remove required coverage checks. Preserve channel/job draining on errors and compatibility with one worker, large framed files and inference mode.

If the measurements do not support a small useful candidate, deliver the breakdown and a concrete explanation instead of inventing a speedup or starting a major architecture change. If a candidate is tested and loses, keep its branch and report it as rejected. The metadata and allocation tracks continue independently.

### 9.4 Admission output

Report the leading merge components, observed scheduling restrictions, A1's precise change, and paired timing/memory results. State whether the remaining bottleneck is serial work, worker execution, scheduling or unresolved. Do not use a maximum batch-wait number as estimated savings.

## 10. Simple measurement protocol

Reuse `bench_index_construction` and the existing result format. Do not create a new benchmark framework or rerun the complete previous profiling campaign for every branch.

At the inspected revision, the supported build and invocation are:

```bash
cargo build --release --locked -p objekte-search-core --example bench_index_construction

target/release/examples/bench_index_construction \
  --root /absolute/path/to/musenalm_extended \
  --alphabet ascii \
  --workers 16 \
  --result-dir /absolute/path/to/experiment_results
```

Replace the example absolute paths with the actual verified corpus and result directories; use the actual resolved worker count rather than assuming 16 on every host. Run each variant from its corresponding checkout, with the binary/commit association recorded. Use `--alphabet unicode` for the secondary comparison and add `--profile` only for diagnostic runs. Use `--check-queries` for separate correctness runs and `--snapshot` for separate persistence/size checks. Do not enable `--plain`, `--jsonl` or `--inference` in the configured-corpus timing comparison.

### 10.1 Comparable controls

- Build release binaries with the same compiler, dependency lockfile and build settings.
- Preserve separate control/candidate executable paths or target directories. Confirm which commit each executable contains before running it; never accidentally compare a stale binary with itself.
- Use the same unchanged Musenalm corpus, schema, alphabet, inference setting, worker count and environment for each pair.
- Use fresh processes. Warm the filesystem cache deliberately; label results warm-cache. Do not drop system caches for this task.
- Reserve the benchmark host; avoid concurrent compilations, tests and other experiment runs. Record CPU/governor information, but do not change system configuration to improve one candidate's result.

### 10.2 Minimum runs

| Comparison | Minimum measurements |
| --- | --- |
| Every candidate versus its specified control | One warm-up per binary, then five paired unprofiled ASCII builds at the normal worker count |
| Metadata scaling | Three paired trials at 1 and 4 workers in addition to the normal-count comparison |
| Unicode check for each experiment | Three paired Unicode trials at the normal worker count |
| Final combined integration versus `B` | Five paired trials for each alphabet at the normal worker count |
| Detailed phase/counter diagnosis | One representative profiled run per variant; repeat only if inconsistent |

Alternate pair order: control/candidate, then candidate/control. Report each raw duration, median, IQR or min/max, and the paired differences. Define delta consistently as candidate minus control, so negative milliseconds indicate improvement.

The primary timing is `build_ns` around repository construction. Whole-process time may be recorded separately, but includes setup and post-build benchmark output. Snapshot serialization and query verification remain outside construction timing. Do not infer build speed from Hyperfine process time alone.

If results are smaller than ordinary variation, classify them as inconclusive or neutral. Add a few trials only when that would settle an integration decision; do not run repeated campaigns until a favorable median appears. There is no mandatory percentage speedup target.

### 10.3 Memory and allocation evidence

- Use existing fresh-process peak RSS measurements, recording the boundary and whether post-build work can affect the peak.
- Report post-build RSS separately where available; allocator-retained pages are not automatically live index memory.
- Record the existing estimated index size or saved snapshot size to detect accidental coverage/storage changes. Snapshot writing remains outside timed construction.
- For E1/E2, cheap opt-in buffer/scratch growth and retained-capacity counters are sufficient supporting allocation evidence.
- For E3/E4, use existing allocation tooling on the same small configured fixture, or focused counters where they actually capture the allocation removed. Keep instrumented runs separate from elapsed-time claims.
- Do not require full-corpus Valgrind, callgrind, perf, flamegraphs or cold-cache runs for routine experiment acceptance.

Use a compact report table:

| Variant | Base/candidate commits | Build median ms | Paired delta ms / % | Discovery ms | Source work ms | Admission ms | Peak RSS MiB | Allocation evidence | Correctness | Decision |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- | --- |

Use `not measured` rather than invented or incomparable values. Record counters separately where their units or fixtures differ.

## 11. Correctness and regression checks

Each implementer runs focused existing tests plus tests for the semantics changed by its track. Avoid duplicating the entire workspace suite on every small iteration. The integrator runs the repository-required workspace checks on the final combined implementation.

Common comparisons on an unchanged corpus:

- Selected files, file IDs, record IDs/slots/ranges and collection memberships.
- Parse counts, malformed/fallback status, diagnostic association and source fingerprints.
- Configured posting/sort/relation counts and equivalent bigram candidate behavior.
- Query digests from the existing construction harness, plus focused cases exercising the modified path.
- Save/load query equivalence and snapshot-format compatibility; byte-identical snapshots are required only where the existing serializer promises deterministic bytes.
- One-worker operation and the normal worker count.
- Small construction smoke checks for an unconfigured collection and JSONL, so the configured-corpus optimization does not break other supported build paths.

Track-specific essentials:

| Track | Required boundary cases |
| --- | --- |
| M | Multiple/overlapping collections, empty collection, metadata failure order, symlink policies, non-UTF-8 path handling |
| E1 | Growing/shrinking file sizes, large then small input, malformed/unreadable input, buffer ownership through callbacks |
| E2 | More than one batch, partial 64-record chunks, changing collection/schema, unused stores, both alphabets, bounded scratch retention |
| E3 | Missing/null/empty, nested and multi-value paths, occurrence order, mapping failure and coverage |
| E4 | Exact numbers, duplicate keys, escapes/Unicode, malformed input, nesting limits, logical array metadata, ordinary versus decoded-scalar reader distinction |
| A1 | Out-of-order completions, small worker pool, tight existing allowance, error draining, framed files and unchanged deterministic admission results |

For final integration run the repository's standard test and Clippy commands, check changed-file formatting and `git diff --check`, and complete relevant CLI/desktop compile checks if the changed interfaces affect them. Shared core changes should automatically benefit CLI and desktop callers; no UI redesign is part of this task.

Correctness must pass regardless of whether an optimization is retained. If an experiment cannot preserve required semantics, report it as rejected rather than reducing the acceptance checks.

## 12. Integration decisions and completion

The supervisor reviews each branch independently before combining changes.

- Integrate the metadata change if it provides a useful measured improvement and preserves its contract. If it fails that goal, report the limitation explicitly; do not label the requested performance fix successful.
- Integrate allocation experiments only when the measured benefit justifies complexity and memory behavior. A clear bounded-memory improvement with neutral speed may be retained with an explicit rationale; lower allocation counts alone are insufficient.
- Keep E4 experimental if its maintenance/correctness cost outweighs its benefit.
- Integrate A1 only if its own evidence supports it. Keep or remove A0 instrumentation according to its usefulness and measured disabled-mode overhead; do not leave uncontrolled diagnostic output in production.
- Validate interactions after integration. Independent savings are not additive; report the combined measured result instead of summing experiment deltas.
- If two changes overlap in the same function, integrate and resolve them deliberately in the supervisor worktree. Do not rewrite the independently measured branch history.
- If a combined result regresses, isolate the interaction with a targeted comparison and omit the problematic combination.

Required deliverables:

1. One reviewed, tested integration branch ready for the user's merge, with retained implementation commits and no changes pushed to `master`.
2. Four independently reviewable allocation experiment branches and their raw simple measurements.
3. Metadata implementation and scaling results.
4. Batch-admission component breakdown and the A1 outcome, or a specific explanation why no bounded candidate was justified.
5. `docs/ai/generated/index_construction_optimization_results.md`, containing the comparison table, acceptance/rejection decisions, exact commits/commands and limitations.
6. Raw evidence under `bench/results/index-construction-optimization/<date>-<baseline>/`, with one directory per track and a README identifying binaries, corpus/configuration, runs and units. Keep artifacts proportionate; do not commit binaries or unnecessary large profiler dumps.
7. This task document moved to `docs/ai/done/` with an appended `Implementation report`, committed and pushed according to the repository workflow. Preserve user edits and the original document's Git state until the required completion step.

Suggested commit sequence:

1. Supervisor: establish baseline and only necessary shared measurement support.
2. M: parallel metadata preparation with focused correctness checks; then measured report.
3. E1–E4 independently: one focused implementation plus relevant checks; then evidence and recommendation on each branch.
4. A0: opt-in admission breakdown and findings.
5. A1: one justified optimization plus comparison against A0.
6. Supervisor: integrate accepted changes, resolve interactions, run final checks and paired measurements.
7. Supervisor: commit final results and completed specification, push, and report the integration commit and worktree status.

Completion is a measured, reviewable outcome, not a requirement that every experiment win. The final report must clearly distinguish implemented-and-retained changes, rejected experiments, inconclusive results and any genuinely incomplete required work.

## 13. Reference links for the inspected revision

- [Profiling report](https://github.com/Simon-Martens/objekte/blob/737cfb7/docs/ai/generated/index_construction_profile.md)
- [Fresh construction and metadata loop](https://github.com/Simon-Martens/objekte/blob/737cfb7/crates/core/src/repository.rs#L1178)
- [File extraction buffer](https://github.com/Simon-Martens/objekte/blob/737cfb7/crates/core/src/source.rs#L1381)
- [Ordinary JSON build extraction](https://github.com/Simon-Martens/objekte/blob/737cfb7/crates/core/src/source.rs#L798)
- [Logical conversion](https://github.com/Simon-Martens/objekte/blob/737cfb7/crates/core/src/model.rs#L370)
- [Bigram scratch and streaming insertion](https://github.com/Simon-Martens/objekte/blob/737cfb7/crates/core/src/bigram.rs#L478)
- [Construction field/text selection](https://github.com/Simon-Martens/objekte/blob/737cfb7/crates/core/src/build.rs#L528)
- [Inline-capable field selection](https://github.com/Simon-Martens/objekte/blob/737cfb7/crates/core/src/reference.rs#L1863)
- [Decoded scalar string retention](https://github.com/Simon-Martens/objekte/blob/737cfb7/crates/core/src/json_scalar_text.rs#L263)
- [Dispatch and ordered admission](https://github.com/Simon-Martens/objekte/blob/737cfb7/crates/core/src/build.rs#L2082)
- [Admission operations](https://github.com/Simon-Martens/objekte/blob/737cfb7/crates/core/src/build.rs#L2287)

Line numbers are navigation aids for the inspected revision. Verify current function boundaries before implementation.

## Implementation report

- **Task ID:** `index-construction-optimization`; completed 12 November 2026.
- **Baseline:** master `737cfb7745ff15915804553ab583af260ac0f2e4`. **Integration:** `integration/index-construction-optimization`, worktree `/home/simon/source/objekte-integration-index-construction-optimization`. The integration code before report delivery is `609474ee42e26c568cd43f89893482e0a0886bbe`. Master was not modified or merged.
- **Implemented:** bounded 64-path-per-worker parallel metadata preparation with ordered insertion; per-batch bounded input-buffer reuse; construction-only direct JSON-to-logical deserialization; opt-in admission component/scheduling diagnostics; and bounded dispatch replenishment after each ordered admission. The source reader, index contents, query semantics, snapshot format, job/byte allowance, and source batch size remain compatible.
- **Independent branches and measured commits:** M `task/index-construction-optimization/metadata-parallelism` (`1b0d4676`, `d5965bd1`); E1 `task/index-construction-optimization/file-buffer-reuse` (`90057337`); E2 `task/index-construction-optimization/bigram-scratch-reuse` (`c6f60bff`); E3 `task/index-construction-optimization/field-selection-allocation` (`e41b7f21`); E4 `task/index-construction-optimization/direct-logical-deserialization` (`66f42cc3`); A0 `task/index-construction-optimization/batch-admission-analysis` (`82f7c1a8`); A1 `task/index-construction-optimization/batch-admission-optimization` (`ce3bd850`). E2 and E3 were measured and rejected, so their code was not integrated. Accepted implementation integration commits: `9cacbe39`, `41b3a68a`, `74584174`, `ed38a27c`, `d91214ed`, `11dd9f5d`; warning cleanup `609474ee`.
- **Validation:** `cargo test --workspace --locked` passed on the combined code, with a final rerun after warning cleanup: 811 passed, 0 failed, 17 ignored across workspace groups. `cargo clippy --workspace --all-targets --locked` passed with existing warnings. The implementation-code diff passed `git diff --check`; the full staged document check allowed the original task source’s intentional Markdown hard-break spaces. Each independent branch passed its core suite and focused semantic tests. Fresh-build queries Q001, Q108, Q116, Q120, and Q194 matched baseline digests, as did parse/index counters. The combined code restored both baseline and new 129,139,458-byte ASCII snapshots with zero source parses and matching query digests.
- **Benchmark:** isolated `cargo build --release --locked -p objekte-search-core --example bench_index_construction` binaries; serial, warm-cache, fresh-process alternating pairs on the configured 148,486-record Musenalm corpus with 16 workers. Five final pairs per alphabet measured `build_ns`: ASCII baseline 2,529.698 ms versus combined 2,107.469 ms, paired median **−422.229 ms (−16.691%)**; Unicode baseline 2,667.664 ms versus combined 2,255.591 ms, paired median **−417.635 ms (−15.655%)**. Each track's exact control/candidate commit, binary hash, command, raw samples, IQR, memory and profile counters are in [the evidence archive](../../../bench/results/index-construction-optimization/2026-11-12-master-737cfb77/README.md). The detailed analysis is [the results report](../generated/index_construction_optimization_results.md).
- **Regressions and limits:** E2 was 46.9 ms ASCII/33.8 ms Unicode slower and retained 14.6 MiB more median ASCII peak RSS; its worker scratch is excluded. E3's configured-corpus time was neutral and its small-fixture allocation saving was negligible; it is excluded. A1's own modest speed gain cost 8.7 MiB ASCII/7.2 MiB Unicode peak RSS within unchanged scheduling limits. The final Unicode peak RSS rose 9.4 MiB while ASCII peak was essentially unchanged; indexed content size did not grow. These are warm-cache results on one `powersave` host. E4's arbitrary-precision serde token handling is covered by focused tests at the locked dependency version. An exploratory strict Clippy run failed on 165 workspace lint findings; standard Clippy passed. No in-scope query or correctness regression remains.
- **Deviation and cleanup:** The task asked for one bounded admission candidate; A1 tests only dispatch replenishment. E2/E3 rejected branches remain independently reviewable. Cleanup completed after report commit `63d02335` was pushed: accepted subordinate and temporary measurement worktrees were removed with normal Git worktree removal, the original untracked task source was removed after its unchanged prefix and remote sync were verified, and this task's temporary memory was deleted. The rejected E2/E3 worktrees and their pushed branches remain independently reviewable; the integration worktree remains for the user's merge. No separate issue document is needed because measured regressions were resolved by excluding the losing experiments.
