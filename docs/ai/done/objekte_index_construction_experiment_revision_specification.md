# Agent specification: correct index construction experiments and repeat benchmarks

Task identifier: `index-construction-experiment-revision`  
Repository: `Simon-Martens/objekte`  
Date: 12 November 2026  
Deliverable: one pushed integration branch containing only justified changes, separate experiment branches, and a reproducible comparison report. Do not merge into master.

## 1. Objective and scope

Complete the design experiments that the first implementation did not adequately evaluate:

1. Contiguous, bounded bigram contributions instead of a linked block arena.
2. Admission parallelism across coarse groups of independent fields and text indexes instead of only two aggregate tasks.
3. SIMD generation of adjacent and skip-one gram keys, rather than SIMD lowercase conversion followed by the original per-byte scalar path.

Retain the accepted direct scalar ASCII folding as the control implementation. Preserve all existing text transformations, Unicode semantics, metadata validation, identity, diagnostics, slot mapping, query results, mutation behavior and snapshot compatibility.

These are performance experiments, not established correctness repairs. A measured rejection is an acceptable result. Do not assume that correcting the experimental design guarantees a speedup.

The previous per-batch join was explicitly required by the original specification. It is not a defect. This task changes the division of work within that boundary; it does not introduce an asynchronous multi-batch admission pipeline.

Do not change discovery, introduce io_uring, weaken validation, change the persistent index representation, prune columns, alter query semantics, or parallelize relationship resolution or unrelated finalization.

## 2. Reviewed evidence and baseline

The reviewed scalar-only integration head was:

`54dcac5c3b3e1ca499180061ebcee7ad60c27101`

Its measured implementation revision was `4f51c45e22ff4329c690fbbb3bad90f18b17a658`. Later changes at the reviewed head were documentation/task completion changes. The former control on master was `ab616dc6aabb9b9fee4657deb6752a6512bec9e5`.

Relevant rejected implementations:

| Candidate | Reviewed revision | Limitation to address |
| --- | --- | --- |
| Linked contribution arena | `78c290475a35e62b738a2b1058da0503470ef056` | Per-column linked traversal, per-block position/link rewriting, no contiguous column spans |
| Two-group parallel admission | `3cecb5884016a837afa01546a2d049a616434bee` | All posting work in one task and all bigram work in another; no field/store decomposition |
| SIMD folding | `d938fa5618` in the ASCII-folding branch history | Vector lowercase output stored to a temporary array and consumed one byte at a time |

The old compact candidate slowed full builds by 34–39% despite lower RSS. Parallel admission regressed at some worker counts and when combined with scalar folding for Unicode. The tested SIMD kernel regressed on common short strings. Treat these as evidence about those implementations.

At task start, fetch refs and record a single immutable control revision B:

- If the accepted scalar implementation is already on current master, use an appropriate current master revision containing it.
- Otherwise start from the completed scalar-only integration revision above, or its verified successor containing the same retained change.
- Confirm that rejected compact and parallel implementations are absent from B.
- Preserve already completed root-relative path/shared-identity and discovery changes.
- Record ancestry and any differences from the reviewed revision. If the code has already implemented one of these corrections, inspect and measure it rather than duplicating it.

All new experiments must start from the same B. Do not use the old pre-scalar control to attribute scalar folding's improvement to a new candidate.

Use the former measurements only as historical context. Recreate B's measurements on the actual benchmark host.

## 3. Agents, branches and worktrees

Read the repository's applicable `AGENTS.md` and `docs/ai/README.md`.

Use one supervisor/integrator and three implementation agents:

| Owner | Branch | Responsibility |
| --- | --- | --- |
| Supervisor | `integration/index-construction-experiment-revision` | Baseline, design review, benchmark scheduling, integration and report |
| Storage agent | `task/index-construction-experiment-revision/contiguous-contributions` | Construction layout, ownership, allocation measurements |
| Admission agent | `task/index-construction-experiment-revision/field-admission` | Disjoint field/store work groups, deterministic errors, scheduling measurements |
| SIMD agent | `task/index-construction-experiment-revision/vector-gram-generation` | Vector key generation, scalar fallback, boundary tests and kernel/build measurements |

Create sibling worktrees under `/home/simon/source`, using names such as `objekte-integration-index-construction-experiment-revision`. Keep the original checkout on master. Do not use `/tmp` for worktrees.

Preserve the earlier experimental branches and reports. Do not rewrite their history. Push after every commit on each branch; do not force-push.

Agents may develop concurrently, but the supervisor serializes all performance runs and prevents competing builds or other heavy jobs on the measurement host. Agents must not edit one another's worktrees.

Agree on interfaces before implementation:

- Storage owns accumulation/contribution representation.
- SIMD owns folded input traversal and gram-key production.
- Admission initially uses B's contribution interfaces.
- Measure candidates independently before adapting and combining retained changes.

## 4. Experiment A: contiguous construction contributions

### 4.1 Required structural properties

The new candidate must have these properties, demonstrated by code inspection and counters:

- A batch's payload for one gram is a contiguous span of words or sparse word entries.
- Column descriptors contain offsets/counts or equivalent ranges, not per-block next links.
- Adjacent and skip-one grams retain their separate meaning and membership.
- Record positions remain batch-local while owned by a local contribution.
- The admitted global base is stored once with the admitted payload/chunk where practical, rather than rewriting every block merely to move the contribution.
- Zero or absent columns remain cheap; no dense whole-repository matrix and no full Unicode-domain table.
- Existing 64-record bitmap aggregation is retained. Do not introduce one allocation or one retained entry per character occurrence.

A suitable minimal sparse representation is a contiguous block array plus a directory of gram-to-range descriptors. Sparse local word positions may be stored in parallel arrays with the u64 words if that avoids padding; an ordinary block array is also acceptable when measured. Dense per-column ranges with implicit positions are optional, not required for this experiment.

Do not reproduce the rejected arena with a different name. A globally contiguous allocation whose individual columns require linked traversal does not satisfy this experiment.

### 4.2 Accumulation and packing

Avoid retaining one growable vector allocation for every gram in every batch and merely copying those vectors into a flat buffer afterward.

Choose one small construction strategy, such as a bounded worker-local aggregation table keyed by gram and local word position, followed by one grouping/packing pass into column spans. Another approach is acceptable if it actually removes the targeted allocation pattern. Do not prescribe a general allocator or build several encodings before measuring the first viable design.

Count and time all costs:

- Accumulation lookups and duplicate-bit OR operations.
- Sorting/grouping and directory construction.
- Packing/copies, scratch clearing, growth and retained capacities.
- Validation, admission and final conversion to the existing Roaring representation.

Prefer moving owned payload allocations into the accumulator. If copying is necessary, report what is copied and how many bytes; do not describe the result as zero-copy.

Reuse worker-local scratch only within a bounded retention policy. A rare large record must not permanently inflate every worker's scratch capacity. Empty contributions should not allocate large tables.

### 4.3 Admission and global ownership

Separate the lifetime of an admitted payload from the lifetime of the extraction batch. Release parsed values, source buffers and unrelated batch state under the existing ownership rules.

If global construction retains compact immutable payload chunks until conversion, those chunks become part of the global construction accumulator. Store their base/count once and maintain suitable column-span references. Do not retain complete extraction batches or add an unbounded pending-work queue.

Global accumulator storage naturally grows with the index. Account for it explicitly; the existing in-flight work bound must still constrain pending extraction and admission work.

Do not move a full rebase/packing pass to finalization and claim it disappeared. Include conversion, destruction and any retained chunk overhead in full-build timing and peak memory.

For non-word-aligned global bases, preserve exact membership. Adding the chunk base when enumerating local set bits is valid. If converting into globally aligned words, handle shifted words, carry bits and overlap by OR correctly; do not assume every batch contains a multiple of 64 logical records.

### 4.4 Validation

Preserve alphabet, coverage, local order, count and overflow checks. Validate descriptor bounds and ensure the representation does not admit accidental overlapping/aliased spans, duplicate column descriptors or unreachable payload entries where its contract prohibits them.

A redundant validation pass may be consolidated only at a documented boundary that preserves the existing checked invariants and failure behavior. Validation must not be skipped merely to improve the candidate.

Test both alphabets against B for:

- Counts 0, 1, 63, 64, 65, 255, 256 and 257.
- Global bases around word boundaries and near the existing slot limit.
- Multi-record files, empty text, fallback-only records and custom text indexes.
- Sparse and dense memberships, repeated grams, many tiny segments and one unusually large record.
- Invalid descriptors, ordering, coverage and arithmetic overflow.

Compare logical column membership and final query behavior, not serialized hash-map iteration order.

## 5. Experiment B: finer admission work groups

### 5.1 Required decomposition

Keep ordered record/inventory admission and deterministic assignment of base/count. Then expose independent work at a finer level than the rejected two-task split:

- Coarse groups of structured posting fields.
- Separate raw/fallback/custom text indexes, or coarse groups of small text indexes.
- Other components only when their dependency and ownership analysis justifies it.

Before coding, document which destination maps, dictionaries, inference state, relation state and sort state each operation touches. A field name alone does not establish independence. Keep genuinely coupled work together.

Use a bounded number of work groups related to available workers and estimated contribution work. Batch record count alone is an inadequate cost estimate: equal record counts can produce very different postings/gram volumes.

Use readily available counts, such as nonempty fields, entries, word blocks or contribution sizes, rather than an additional expensive scan. Keep small work inline. Do not spawn a task per record, gram or tiny field.

Maintain one join at the end of the batch. Do not change admission order, increase queue limits, restore the previously rejected dispatch-replenishment experiment, or introduce a second worker pool.

### 5.2 Ownership and errors

Prepare any required destination entries on the coordinator before borrowing independent destinations. Prefer safe scoped APIs and disjoint mutable iterators/slices or owned work items. Include preparation and task-list allocation in admission timing.

Do not use a global per-update mutex, concurrent insertion into the same ordinary map, unsynchronized bitmap writes, or broad unsafe Send/Sync implementations.

Execute only operations that are safe to run speculatively within an unpublished build. Join/drain all started work before returning an error. Preserve the established component error precedence and within-component order where defined; choose the error by that order, not task completion order.

Add a deliberate failure test with errors in two independently scheduled components to establish deterministic selection. Also cover single-worker progress, tiny batches, absent components, repeated builds, and mixed logical/fallback/custom contributions.

### 5.3 Explain actual waiting

The report must distinguish:

| Interval | Meaning |
| --- | --- |
| Ready-to-admission delay | Batch is available but ordered admission has not started |
| Ordered record admission | Coordinator establishes records, identity and global slots |
| Task preparation/submission | Work items are built and offered to the existing pool |
| Submission-to-start delay | A merge group has been submitted but has not begun execution |
| Group execution interval | Elapsed time within a merge group; not measured CPU time |
| Merge/join interval | Elapsed interval until required batch merge work completes |
| Remaining admission work | Sequential work after the merge groups |

Use coarse opt-in timestamps and existing diagnostics. Do not add per-record clocks or atomics to ordinary builds. Establish real task overlap from start/end intervals.

Component intervals may overlap. Do not subtract their sum from wall time, add them to claim total CPU time, or call maximum batch queue delay recoverable time.

Use the measurements to distinguish task imbalance from scheduling delay and contention with extraction. More active CPUs is not an acceptance criterion.

## 6. Experiment C: SIMD gram generation

### 6.1 Scope of the revised candidate

Keep the accepted scalar implementation as both the control and portable fallback.

The revised vector path must do useful gram work in addition to lowercase conversion: form adjacent and skip-one pair inputs/keys and their validity masks in vectors for eligible ASCII runs. Do not simply store lowercase bytes and call the old scalar byte sink once per byte.

A small stack buffer of already formed keys/masks is allowed if the existing accumulator still requires scalar inserts. Measure that materialization and insertion cost. Do not require unsafe vector scatter or shared writes. Describe precisely what is vectorized and what remains scalar.

Use the exact existing key encoding. ASCII-index printable-byte filtering and Unicode-index scalar-pair keys are different contracts. A vector implementation for the ASCII alphabet with a scalar Unicode-alphabet fallback is acceptable; report backend coverage honestly and benchmark both modes.

Do not add new text normalization or a folded heap string. Preserve borrowed input and the existing Unicode fallback.

### 6.2 Boundary and Unicode behavior

For bytes 0–127, folding changes only A–Z under the locked full non-Turkic policy. High-bit input must be decoded as complete UTF-8 scalars and passed through the existing full fold, including expansions.

Preserve the preceding two folded units across vector chunks, scalar tails and Unicode transitions. The units are UTF-8 bytes for the ASCII index and Unicode scalars for the Unicode index. Reset only at the established segment boundary.

Unsupported ASCII-index bytes still occupy positions in the folded stream. In particular, skip-one eligibility depends on the two endpoints; an unsupported middle byte must not accidentally remove a valid skip-one gram.

Do not reset history on punctuation/control/non-ASCII input merely because it is not an eligible ASCII endpoint. Do not join separate values or segments.

Guard loads by actual slice bounds, including tails. Handle vector lane boundaries and the first two positions of each block. Preserve the scalar path for short or unsuitable runs without a separate whole-string ASCII prepass.

Use the repository's existing runtime CPU feature conventions. Do not require AVX2 for the portable binary. Keep any unsafe intrinsics small and auditable. Implement and validate the available architecture first; do not ship an uncompiled/unexecuted additional backend as completed work.

### 6.3 Required tests and measurements

Differential tests must compare the folded result, emitted gram sequence or equivalent precisely defined emission contract, and final memberships against the scalar oracle.

Cover all ASCII byte values; empty and 1/2/3-byte segments; lengths around 8/16/32/64-byte boundaries; every relevant input alignment; Unicode expansions; mixed runs; non-ASCII input at chunk boundaries; segment boundaries; and adjacent/skip-one carry state. Force scalar and SIMD backends independently in tests on supported hardware.

Use two focused measurements:

1. Fold and emit grams into an observable checksum, for continuity with the old kernel.
2. Feed the same representative transformed segments into the real contribution builder, including insertion, duplicate handling and builder completion.

The second measurement is required: a checksum does not represent hash-map/bitmap insertion cost.

Include short lengths 0–8 and 16/32/64/128, plus 1 KiB and longer text; ASCII, ASCII-heavy mixed UTF-8 and predominantly non-ASCII cases; repeated and varied grams. Avoid a large Cartesian benchmark suite. Prefer a small corpus reflecting measured transformed-segment length/content distributions, supplemented by boundary cases.

If the revised kernel/real-builder experiment clearly loses, reject it without forcing a full production integration. A candidate considered for retention must pass the full-build comparison.

## 7. Benchmark protocol

Use the existing `bench_index_construction` example and comparison driver. Extend their diagnostics only where necessary; do not introduce a new framework.

### 7.1 Fixed conditions

Record B and candidate commits, binary hashes, compiler, locked dependencies, release settings, CPU model/topology, worker count, affinity, governor, corpus/schema fingerprints and exact commands.

Use the same Musenalm corpus and schemas for each paired comparison. Record file, logical-record and source-byte counts. Do not compare older Ryzen results with newer Intel runs as one speedup.

Use fresh processes and the same warm-cache policy as before, one warm-up per role, alternating control/candidate order, and separate target directories. Keep profiling and allocator instrumentation disabled for ordinary timing.

Do not overlap timing runs with agent builds or other benchmarks.

### 7.2 Small, predetermined run plan

For each viable independent candidate:

- Five alternating paired full builds against B for ASCII and five for Unicode at the normal resolved worker count.
- Preserve every valid pair and publish its raw values.
- If a retention decision remains inconclusive, append four pairs to that same comparison for nine total. Do not replace the original five with a more favorable series.
- If still inconclusive, classify the speed result as inconclusive and make the retention decision based on complexity and independently established memory benefits.

For admission additionally use three paired builds in each alphabet at 1, 4, 8 and 16 workers where available, omitting duplicate settings already covered by the normal count. These settings matter because the previous candidate regressed at low/intermediate counts. Record any gating behavior explicitly.

Do not tune thresholds separately to every run. Choose a small rule from coarse work estimates and evaluate that same rule across the matrix. A worker threshold must not enable a configuration already shown to regress without a new measured justification.

SIMD kernel timings must vary measurement order and make output observable. Report scalar/native differences separately by length/content group, not just an aggregate dominated by long strings.

### 7.3 Measurements to report

For ordinary builds report full `build_ns`, all trial values, paired deltas, paired median, win/loss count, spread, peak RSS and post-build RSS. Keep arithmetic differences of medians distinct from medians of paired differences.

Use separate diagnostic runs for phase timings. Include packing, conversion and destruction in the end-to-end boundary; relocating work does not remove it.

For storage, collect one comparable focused allocation profile for B/candidate and opt-in full-corpus storage counters:

- Allocation count, gross allocated bytes and peak live bytes, with scope clearly identified.
- Actual type sizes, lengths/capacities and descriptor/payload/scratch bytes.
- Global accumulator, active workers, ready/channel contributions and retained chunks where observable.
- Copy/rebase work and the number of allocations retained after consumption.
- Final persistent index size separately from construction storage.

If counters omit structures, label the totals as lower bounds. Do not sum independently observed peaks as though simultaneous. RSS, allocated capacity and live payload bytes are different measurements; explain their disagreement rather than selecting only the favorable one.

For admission, collect one separate profile for a normal-worker build and one representative regression or low-worker case. Record coarse work-group sizes, execution overlap and scheduling delays.

## 8. Correctness and integration gates

Run focused tests while developing. Before proposing any retained combination, run the repository's required checks and applicable workspace tests, query comparison, lifecycle and snapshot-restore validation.

Use the existing extended Musenalm suite (194 cases at the reviewed revision, or its current superset). Compare counts and ordered SearchHit values, including identity, source paths/ranges, status, diagnostics, matched fields and payload, for both alphabets.

Restore B and candidate snapshots under the candidate in required-load mode and verify no rebuilding. Preserve final logical index contents and format/version contracts. Serialization byte order need not match if the existing contract permits nondeterminism.

Preserve original validation/error behavior. Do not remove metadata checks, contribution checks or identity handling to make a benchmark faster.

Report unrelated baseline check failures separately, with evidence. Do not conceal newly introduced failures behind existing warnings or formatting drift.

## 9. Retention and combined measurements

Review structural compliance before interpreting performance. The supervisor must be able to demonstrate:

- Storage: contiguous per-column spans, no per-block links, bounded scratch/in-flight work, and explicit base/ownership handling.
- Admission: independently scheduled field/store groups, safe exclusive ownership, unchanged batch ordering and deterministic errors.
- SIMD: vector formation of gram keys/masks rather than only byte lowercasing, with measured real-builder behavior.

Keep a candidate only when it delivers a useful repeatable full-build improvement at acceptable memory/complexity cost. A storage change may also qualify through a meaningful measured live-memory/allocation improvement with neutral build performance.

Treat small differences inside the observed spread as inconclusive. Investigate or reject repeatable regressions in either alphabet or enabled worker configurations; do not average them away with improvements elsewhere. The user's primary objective is construction speed.

Merge retained candidates into the integration worktree one at a time. Measure the final combination against B with five paired builds per alphabet. If interaction changes the decision, perform a direct comparison with and without the suspect candidate using the same protocol. Do not sum independent gains.

Keep scalar folding even if all new candidates fail. Preserve rejected branches and evidence. No new optimization is preferable to a slower or unjustifiably complicated integration.

## 10. Phases, commits and deliverables

1. Supervisor: record B, inspect current source, coordinate interfaces and commit the benchmark manifest/protocol.
2. Each agent: implement its corrected experiment and focused differential tests; commit and push an inspectable candidate.
3. Each agent with supervisor: complete the small independent measurements, commit and push results and any justified adjustments.
4. Supervisor: review structure, correctness and measurements; integrate only accepted candidates.
5. Supervisor: run combined validation/measurement; commit and push the final report and completed task document.

Save the concise report to:

`docs/ai/generated/index_construction_experiment_revision_results.md`

Save commands, manifests and raw evidence following existing archive conventions under:

`bench/results/index-construction-experiment-revision/<date-and-baseline>/`

On completion, move the assigned task document to:

`docs/ai/done/objekte_index_construction_experiment_revision_specification.md`

Preserve its original instructions/user comments and append the implementation outcome. Follow repository rules for task documents and temporary memory. Stage explicit task-owned paths; preserve unrelated uncommitted user work.

The report must identify each branch/commit, what changed relative to the rejected prototype, whether the corrected design was actually tested, all retained/rejected decisions, normal/low-worker results, allocation findings, backend coverage, validation and limitations.

Deliver one pushed integration branch ready for the user to review and merge. Keep master unchanged. Missing required validation, failed pushes or unresolved correctness failures are blockers, not completed work.

## 11. Source references

- [Reviewed integration and result report](https://github.com/Simon-Martens/objekte/blob/54dcac5c3b3e1ca499180061ebcee7ad60c27101/docs/ai/generated/index_construction_data_path_results.md)
- [Accepted scalar implementation](https://github.com/Simon-Martens/objekte/blob/54dcac5c3b3e1ca499180061ebcee7ad60c27101/crates/core/src/bigram.rs)
- [Rejected linked contribution arena](https://github.com/Simon-Martens/objekte/blob/78c290475a35e62b738a2b1058da0503470ef056/crates/core/src/bigram.rs)
- [Rejected two-group admission implementation](https://github.com/Simon-Martens/objekte/blob/3cecb5884016a837afa01546a2d049a616434bee/crates/core/src/build.rs)
- [Earlier SIMD implementation](https://github.com/Simon-Martens/objekte/blob/d938fa5618/crates/core/src/bigram.rs)


## Implementation report

Task `index-construction-experiment-revision` completed 12 November 2026 on `integration/index-construction-experiment-revision`, based on reviewed scalar control `54dcac5c3b3e1ca499180061ebcee7ad60c27101`. The integration worktree is `/home/simon/source/objekte-integration-index-construction-experiment-revision`; master remains at `ab616dc6aabb9b9fee4657deb6752a6512bec9e5` and was not modified. The detailed comparison, commands, raw measurements and limitations are in [the result report](../generated/index_construction_experiment_revision_results.md) and its [evidence archive](../../../bench/results/index-construction-experiment-revision/2026-11-12-b54dcac5/README.md).

### Implemented outcome and contributing branches

- Retained the accepted scalar ASCII folding in B and the new bounded, contiguous per-column bigram contributions. They hold local 64-record bitmap positions in packed spans, move owned payload chunks into the global accumulator with one admitted base per chunk, and preserve validation and final Roaring conversion. Storage implementation commits are `696ac601e0`, `d7534b6d90`, `9341f93497` on `task/index-construction-experiment-revision/contiguous-contributions`, merged into integration by `db2834e17f` and `dbbf495733`; `ceebc48811` removes an unused admission estimate from retained code.
- Implemented and tested coarse independent posting-field/text-store admission on `task/index-construction-experiment-revision/field-admission` (`b38d6696e4`, revised `612afa1f3e`, test follow-up `b01aea540c`). The revised gate grouped all 262 dominant `contents` batches, but 4- and 16-worker full builds regressed. Its code remains on the pushed experiment branch and was not merged.
- Implemented and tested AVX2 adjacent/skip-one gram key and validity-mask formation on `task/index-construction-experiment-revision/vector-gram-generation` (`3b155e13`, evidence head `6a4ad83093`). It retains scalar insertion and a Unicode-alphabet fallback. Nine-pair full builds did not show a repeatable two-alphabet benefit; its code remains on the pushed experiment branch and was not merged.

### Validation and performance

`cargo test --workspace --locked` passed 820 tests with no failures (18 ignored). `cargo clippy --workspace --all-targets --locked` exited successfully with warnings also present at B. `scripts/format-rust-spacing.py --check` and `cargo fmt --all -- --check` each fail on both B and retained code at the same unchanged `crates/cli/src/execution.rs` formatting drift; baseline and candidate logs are preserved. `git diff --check B...HEAD` passed. Focused candidate tests, including storage boundaries/invalid descriptors, admission error precedence and forced SIMD backends, passed on their branches.

The benchmark command was the locked release `bench_index_construction` example driven by `bench/results/index-construction-optimization/2026-11-12-master-737cfb77/compare.py` using the canonical Musenalm root, one warm-up per role, fresh processes and alternating paired builds. The final retained integration code at `ceebc48811` won every one of five ASCII and five Unicode full-build pairs against B at 32 resolved workers. Paired median build differences were −162.775 ms (−15.036%) ASCII and −139.141 ms (−11.944%) Unicode; median peak RSS fell from 585.605 to 507.945 MiB and 601.477 to 524.125 MiB, respectively. Admission was also measured in three pairs per alphabet at 1, 4, 8 and 16 workers; its revised design regressed by 8.961% ASCII and 10.227% Unicode at four workers. Full trial values, spreads, memory/DHAT profiles and SIMD kernel/real-builder results are in the report and archive.

Fresh B and retained builds matched all 194 extended Musenalm `(count, ordered SearchHit list)` query digests in each alphabet. B and retained snapshots for both alphabets restored under the candidate in required-load mode with `build_parses=0`, and all 194 digests matched fresh results. Snapshot payloads are kept locally under the integration worktree's ignored `target/index-construction-experiment-revision/snapshot-payloads/`; committed manifests include hashes and sizes.

No approved scope deviation or unresolved in-scope correctness failure remains. The observed formatting failure is unchanged from B. The performance finding is limited to the measured host, corpus, schema and worker matrix. Experiment branches and raw evidence are preserved. Subordinate worktrees and temporary task memory are to be removed after this completed document and report are pushed; the integration worktree and branch stay for review. No issue report was necessary.
