# Agent specification: SIMD folding, compact bigram contributions and parallel admission

Task identifier: `index-construction-data-path`  
Repository: `Simon-Martens/objekte`  
Decision date: 12 November 2026

## 1. Objective

Implement and evaluate three construction improvements:

1. Process ASCII bytes directly, including ASCII runs inside mixed UTF-8 segments; compare scalar and SIMD implementations while retaining full Unicode case-folding semantics.
2. Replace fragmented per-gram batch storage with compact, bounded construction contributions.
3. Merge independent index contributions concurrently after ordered record admission establishes their global slot mapping.

Deliver one tested integration branch containing the useful changes and a concise measurement report. Implement all three experiments, but reject candidates whose actual cost outweighs their benefit. Do not retain an unsuccessful optimization merely because this document describes it.

Preserve text transformations, metadata validation, identity, index coverage, diagnostics, source ownership, exact query results and the final persistent index contract. This task changes the construction path, not the meaning of indexed text.

Do not merge into or push changes to `master` for this task. Earlier authorization to merge the selected optimization experiments applies to that separate task.

## 2. Baseline and existing work

The inspected code is at `7a84a368152ce018fc1e8cd0e297032bf8c0ea02` on `integration/index-construction-optimization`. That historical branch includes dispatch experiment A1, which the selected-merge specification removes.

The intended baseline includes:

- Bounded parallel metadata preparation, including the 64-path-per-worker window.
- Bounded source input-buffer reuse.
- Direct JSON deserialization into the existing logical value model.
- Opt-in admission component/scheduling diagnostics.
- Removal of A1's dispatch replenishment between individual ordered admissions.
- Continued exclusion of the rejected retained-bigram-scratch and inline-field-selection experiments.

Fetch current remote master, inspect the actual implementation and record a fixed baseline revision B. Prefer the completed selected-merge result on master. If that task is still underway, use its completed, pushed candidate when available and document the ancestry; do not execute or silently supersede its task. If no selected baseline is available, record that dependency rather than benchmarking against the unadjusted historical branch as though the prior instructions had completed. Independent kernel design and test preparation may proceed.

Preserve the separate root-relative path/shared-record-ID changes if they have landed. They are not part of this task. Compare all candidates against the same actual B and dataset; do not count another task's improvements as this task's result.

The prior profiles put bigram admission around 450–480 ms and structured posting admission around 420–460 ms. Those figures motivate the work; they are overlapping pipeline component measurements, not a promised saving. Long batch wait intervals are not recoverable-time estimates.

## 3. Agent and branch organization

Read applicable `AGENTS.md` and `docs/ai/README.md`. Use a supervisor/integrator and three implementation agents, each with an isolated worktree. The user is assigning this specification for execution; its presence alone in a task directory is not authorization to start other tasks.

| Owner | Branch | Responsibility |
| --- | --- | --- |
| Supervisor | `integration/index-construction-data-path` | Baseline, interface coordination, serialized measurements, integration, reports |
| Agent 1 | `task/index-construction-data-path/ascii-folding` | Scalar mixed-text fast path, SIMD candidate, equivalence tests |
| Agent 2 | `task/index-construction-data-path/compact-contributions` | Construction representation, allocation evidence, coverage validation |
| Agent 3 | `task/index-construction-data-path/parallel-admission` | Scoped component/field merging, deterministic errors, profiling semantics |

Use sibling worktrees under `/home/simon/source`, with the repository's standard names, for example `/home/simon/source/objekte-integration-index-construction-data-path`. Keep the original checkout on master. Never use `/tmp` for these worktrees.

Start the experiment branches at recorded B so their effects can be compared independently. Agents 1 and 2 both touch `bigram.rs`; agree their boundary first: Agent 1 changes folded-stream production and gram emission, while Agent 2 changes accumulation/contribution storage. Each works only in its own checkout. The supervisor merges their changes deliberately.

Agent 3 initially targets B's contribution interface. After integration, adapt it to the compact representation if that candidate is retained. No agent edits another agent's worktree.

Push immediately after each commit on its respective branch. Preserve unrelated files and user task-document edits; stage explicit task-owned paths. Do not rewrite published history or force-push. Compile and benchmark without competing heavy jobs; parallel agent work does not authorize concurrent timing runs.

## 4. Experiment 1: direct ASCII folding and SIMD

### Semantic basis

The existing contract is full, non-Turkic case folding using the repository's locked `unicode-casefold` implementation. At the inspected revision, `FOLDED_REPRESENTATION` identifies Unicode 9.0.0 data.

For input bytes 0–127, this policy changes only `A`–`Z` to `a`–`z`. All other ASCII bytes remain unchanged by folding. This rule can be hardcoded; a separate whole-string `is_ascii()` pass is not required.

Default case folding is context-independent under this fixed policy. Therefore ASCII runs may use the direct rule while non-ASCII scalars use the existing full-fold iterator. Do not substitute locale-sensitive lowercasing, `char::to_lowercase`, normalization, a different Unicode version, or simple folding.

The complexity is preserving the folded stream and its pair boundaries, not the ASCII lowercase operation itself.

### Required scalar implementation

Implement a scalar byte path that classifies input as it is consumed:

1. Consume ASCII runs directly, lowering only bytes in `A..=Z`.
2. At a non-ASCII byte, decode a complete UTF-8 scalar from the valid input string.
3. Emit every scalar produced by the existing full-fold operation, including expansions.
4. Resume the ASCII path without resetting adjacent/skip-one history.

Keep one logical folded stream across ASCII runs, Unicode scalars, expansion output and processing blocks. Reset pair history only at the existing semantic segment/record boundaries.

Use borrowed input. Do not reconstruct a complete owned folded string merely to obtain the ASCII shortcut. The unchanged reference iterator remains the correctness oracle in tests, not a second production implementation that every build executes.

### SIMD candidate

Add a measured SIMD implementation of ASCII classification/lowercasing:

- x86-64: AVX2 with runtime feature detection and a portable fallback.
- AArch64: NEON where the target guarantees/supports it; otherwise the portable path.
- Keep dispatch outside the per-byte loop and avoid repeated setup where practical.
- Inspect high bits in vector blocks to identify ASCII spans or all-ASCII blocks. Handle mixed blocks and scalar tails without a separate mandatory full-input scan.
- A vector operation may leave non-ASCII bytes untouched, but this does not replace their Unicode decoding/folding.
- Use bounded temporary storage, such as a stack-sized vector block or a capped worker buffer, if materializing normalized bytes improves throughput. Do not require a buffer proportional to the largest source file.
- Preserve input buffers; they may also serve parsing, provenance or other representations.
- Never read beyond the slice, split UTF-8 decoding at an arbitrary byte boundary, use unsupported instructions, or introduce unsafe concurrent writes.

First compare direct scalar processing against SIMD processing on the same transformed segments. Keep SIMD only where it improves the complete operation, including classification, buffer writes, pair generation and boundary handling. A short-input scalar threshold is acceptable if measurements support it. Do not borrow FFF's 1 KiB threshold without measuring our segment distributions.

Implement and test native SIMD on available hardware. If another architecture cannot run locally, record compile checks and the runtime-test limitation; do not claim an unexecuted backend passes. A portable fallback remains mandatory.

### Alphabet-specific invariants

The two alphabets consume the same full-folded text but form different units:

- ASCII: form pairs over the UTF-8 bytes of that folded stream. Only printable-byte pairs are indexed; unsupported bytes still occupy positions.
- Unicode: form pairs over folded Unicode scalars, using the existing exact key encoding. Non-ASCII and control characters remain meaningful scalars.

Do not copy FFF's zero-sentinel masking into the Unicode path. Do not discard nonprintable bytes before forming ASCII windows.

Examples to test:

| Input segments | Required property |
| --- | --- |
| `["ABC"]` | Adjacent `ab`, `bc`; skip-one `ac` |
| `["a\nb"]` | ASCII: no adjacent printable pair, but skip-one `ab`; Unicode retains newline-containing adjacent pairs |
| `["ab", "cd"]` | No pairs crossing the segment boundary |
| `["aéb"]` | Unicode has skip-one `ab`; ASCII does not, because UTF-8 `é` occupies two byte positions |
| `["AßB"]` | Full-folded stream is `assb`, including pairs across expansion boundaries |
| `["K", "İ", "Σςσ"]` | Match the locked full-fold oracle, including non-ASCII-to-ASCII output and multi-scalar expansion |

Use meaningful differential tests for both alphabets, adjacent and skip-one grams, empty/one-character inputs, every ASCII byte, punctuation near `A..Z`, deterministic mixed-Unicode samples and vector boundaries. Include lengths and boundary placements around 15/16/17, 31/32/33 and 63/64/65 bytes. Compare scalar, native SIMD and reference folded streams/gram sets. Exercise bigram records around 63/64/65 slots and multiple segments.

No gram version or query-semantic change should be necessary. Verify that construction remains compatible with the existing query-side folding.

## 5. Experiment 2: compact construction contributions

### Current cost

The inspected builder already accumulates 64 record bits per gram. Do not undo that aggregation or assume there is an allocation per text occurrence.

The remaining construction representation is `HashMap<u64, Vec<GramBlock>>`, separately for adjacent/skip grams and each relevant text index. A block stores `first_slot: u32` and `bits: u64`; normal alignment commonly makes it 16 bytes. Per-gram vectors and dictionary entries add allocation and capacity overhead. Serial admission validates, rebases and appends those blocks.

This experiment changes that representation. It does not repeat the rejected experiment that merely retained more old scratch tables between jobs.

### Required design direction

Use bounded contiguous storage for batch contributions with compact column descriptors. Prefer:

- A batch record count and a single global base assigned at admission.
- Local record positions until consumption requires global positions.
- A column directory identifying each gram's region in shared block/word storage.
- Dense local word ranges where occupancy justifies them, and sparse local word positions where dense zero padding would be wasteful.
- Direct lookup for the bounded ASCII pair domain where beneficial; sparse lookup for Unicode's actual scalar-pair keys.

Choose the smallest design that demonstrates a benefit. Do not build a general allocator, global interner or permanent multi-format query engine. A uniform sparse flat layout is acceptable if it wins; an adaptive encoding is not mandatory.

A dense 512-record local column needs eight u64 words with implicit positions. This is an example measured in records, not a claim that every 512-file batch has 512 records. Multi-record files and variable batch sizes must remain supported.

Retain a batch base once rather than eagerly adding it to every stored block where practical. Check overflow, coverage and order before publication. Applying an offset later is still work and must be included in timings.

Avoid a full-record-domain dense matrix for every field, especially in Unicode mode. Do not allocate all possible Unicode columns. Empty raw/fallback/custom indexes should remain cheap.

Moving old per-gram vectors into a flat buffer only at the end, after paying all original allocation costs, is not enough unless measurements show a useful net reduction elsewhere. Include packing, sorting, clearing and directory construction in measured cost.

### Lifetime, bounds and final representation

Consumed source contributions should release or transfer ownership promptly. Do not solve serial admission by retaining every batch until finalization. If retaining compact chunks is necessary, account for all live chunks, descriptors, capacities and queues; compare their peak footprint with B.

Preserve the existing scheduling allowance and apply equivalent accounting to any newly retained intermediate work. Do not increase job limits or silently bypass memory bounds.

Keep final `BigramIndex`, Roaring/query behavior and snapshot format unchanged unless a narrowly necessary internal adjustment demonstrably preserves their external contract. No column pruning, alphabet change, query fallback policy change or permanent dense index is authorized.

Keep metadata, identity and contribution validations. Consolidating a redundant pass is allowed only when the same invariants are enforced at a documented boundary, with equivalent errors and no unchecked input reaching publication.

Test equivalent column membership for both alphabets, no text, fallback-only records, custom fields, sparse/dense distributions, non-word-aligned batch bases and counts 0/1/63/64/65/255/256/257. Include multi-record files, many tiny segments and an unusually large/random-text record. Validate coverage, ordering and overflow/error behavior with the existing construction oracle.

Measure allocations/allocated bytes and peak live contribution storage separately from final index bytes. Report actual `size_of`, capacities and meaningful allocation evidence; do not infer an RSS saving from a struct-size calculation.

## 6. Experiment 3: parallel admission

### First implementation boundary

Retain ordered record/inventory admission. It establishes deterministic record slots, IDs, diagnostics and each batch's global base/count.

After that step, execute independent index merges concurrently on the existing Rayon pool. Begin with scoped parallel work inside one admitted batch, join it, then advance to the next batch.

Candidate work groups are bigram contributions and coarse groups of structured posting fields. Include custom indexes or other components only where their actual dependencies permit it and their measured cost warrants a task. Preserve existing inference, relation, sorting and structural-fact dependencies. Relationship resolution and unrelated finalization changes are outside scope.

Do not introduce a second full worker pool, a new asynchronous pipeline or A1 dispatch replenishment. Do not schedule a task per gram or per record.

### Ownership and correctness

- Give each task exclusive ownership or a disjoint mutable borrow of its destination indexes/fields. Prefer safe scoped Rust/Rayon APIs.
- No per-update global mutex, shared mutable hash-map insertion, unsynchronized bitmap OR, or broad unsafe `Send/Sync` workaround.
- Different record IDs can share a bitmap word. Record-level distinction alone does not justify concurrent writes to the same storage.
- Keep the same batch ordering within each index. Join required work before the next batch is admitted.
- Validate all relevant results and preserve the pre-change precedence of component errors and any established within-component error ordering. Do not select an error based on which worker finishes first.
- On error, join/drain started work, discard the unpublished build as required and preserve the existing failure contract. Do not expose a partially admitted repository.
- A one-worker configuration must complete without deadlock. Prefer an inline path for one worker and small contributions.
- Avoid waiting on a bounded channel from a worker when progress requires another task on the same exhausted pool.
- Keep queue/job/byte bounds unchanged and account for scopes holding extracted contributions alive.

Use coarse work groups and a small, justified threshold. Account for contention with still-running extraction jobs on the same pool. Increased apparent CPU activity alone is not success.

Agent 3 measures this independently using B's layout. The supervisor repeats relevant tests after combining it with Experiment 2.

### Profiling must describe parallel work correctly

A0's formerly sequential component durations can overlap after this change. Update the diagnostic model and report labels accordingly:

- Keep an elapsed admission wall-time interval around the complete admission operation.
- Record ordered record-admission time and elapsed parallel-merge/join interval.
- Record component service durations separately; their sum is not admission wall time or measured CPU time.
- Do not compute wall-time remainder by subtracting overlapping component durations from total elapsed time.
- Keep diagnostics opt-in; do not add per-record clocks or atomics in ordinary builds.
- Record enough coarse task/group information to establish that actual merges overlap. Keep this separate from unprofiled performance trials.

Extend existing tests with deterministic error selection, repeated builds with identical slot/ID mappings, small/single-worker operation, mixed logical/fallback batches and integration of several posting/text indexes. Reuse existing query and lifecycle tests where they already cover semantics.

## 7. Measurement plan and retention decisions

Reuse `bench_index_construction` and the existing comparison driver; do not create a new benchmark framework.

### Independent comparisons

For each experiment's preferred candidate, run one warm-up per role and three alternating B/candidate fresh-process pairs for ASCII and three for Unicode on the configured Musenalm corpus. Use release builds, locked dependencies, fixed worker settings and separate target directories. Record exact revisions, binary hashes, corpus/schema fingerprints, host, compiler and commands.

For Experiment 1, compare reference, scalar fast path and SIMD using a small kernel benchmark that includes full gram emission and representative transformed segments. Cover short/long ASCII, ASCII-heavy mixed UTF-8 and predominantly non-ASCII text. Perform the full-build comparison on the preferred implementation; add a direct full-build scalar/SIMD comparison only if needed to select the production path.

For Experiment 2, use one focused allocation measurement for B and candidate on the same fixture, plus opt-in contribution storage/capacity counters on the full corpus. Prefer existing DHAT/allocator instrumentation when available; never time a profiler-instrumented process as the normal build. Label process-wide allocation totals accurately.

For Experiment 3, use one separate profiled build to verify overlapping work and revised timing accounting. Run focused one- and four-worker correctness/scaling checks in addition to the ordinary available-worker comparison; avoid a large tuning matrix.

Across all candidates, report:

- Full `build_ns`, individual trial values, medians and paired deltas.
- Peak RSS and post-build RSS, with units.
- Relevant extraction/bigram/admission service and elapsed intervals from separate profiles.
- Allocation/block counts and live intermediate bytes where measured.
- Final index counters and payload size, which should preserve semantics.

Inspect three pairs before adding runs. Extend an inconclusive comparison to five pairs only if its outcome changes a retention decision. Do not repeat all experiments because one needs another measurement.

### Retention

A faster microbenchmark is insufficient if full builds regress. Keep clear, useful improvements with reasonable complexity and memory cost. A time-neutral compact representation may be retained for a demonstrated meaningful allocation/live-memory improvement without material regressions.

Reject marginal speed gains accompanied by disproportionate memory growth or complexity. Do not sum independent savings. Record rejected candidates and preserve their branches/evidence.

After merging retained changes, perform the same short B/combined comparison for both alphabets. This detects interactions between folded-stream speed, contribution layout and pool scheduling.

## 8. Integration validation and completion

Before final validation, integrate any relevant master advancement in the integration worktree, preserving the selected baseline changes and concurrent path/identity work. If this changes the comparison base or implementation, record it and repeat only affected checks/measurements.

Run repository formatting checks, then:

```bash
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked
```

Use existing warning policy; do not turn this task into unrelated lint cleanup. Run the existing query suite and compare result membership/order/status with B under the same path contract. At minimum reproduce the archived Q001, Q108, Q116, Q120 and Q194 checks for both alphabets. Include Unicode-expansion and segment-boundary queries beyond that fixed corpus if absent from the suite.

Verify baseline and candidate snapshots restore under the candidate in required-load mode, without rebuilding, and return equivalent results. Compare logical index contents rather than nondeterministic serialized hash-map byte order. Run relevant lifecycle tests to ensure construction-only changes do not alter mutation/restore semantics.

The supervisor reviews final code, including CPU dispatch, bounds, ownership, deterministic failures and diagnostics. No new dependency is expected; prefer existing crates and standard architecture intrinsics.

Save:

- Results: `docs/ai/generated/index_construction_data_path_results.md`.
- Raw evidence: `bench/results/index-construction-data-path/<date-and-baseline>/`, following existing archive conventions.
- Completed specification: `docs/ai/done/objekte_index_construction_data_path_specification.md`, with appended implementation report.

The report identifies B, branch/commit for each experiment, scalar/SIMD backend coverage, retained/rejected decisions, combined measurements, validation, limitations and completion revision. Preserve original task content and user comments under the repository lifecycle rules. Commit and push the completed specification and results. Keep temporary memory out of Git.

Deliver the pushed integration branch and worktree, with master unchanged. Required correctness failures, unperformed required validation or failed pushes are blockers, not successful completion. A measured rejection of an optimization is a valid experiment outcome.

## 9. Source references

- [Inspected Objekte bigram builder](https://github.com/Simon-Martens/objekte/blob/7a84a368152ce018fc1e8cd0e297032bf8c0ea02/crates/core/src/bigram.rs)
- [Inspected Objekte admission implementation](https://github.com/Simon-Martens/objekte/blob/7a84a368152ce018fc1e8cd0e297032bf8c0ea02/crates/core/src/build.rs)
- [Existing measurements and comparison procedure](https://github.com/Simon-Martens/objekte/blob/7a84a368152ce018fc1e8cd0e297032bf8c0ea02/docs/ai/generated/index_construction_optimization_results.md)
- [FFF builder, SIMD normalization and disjoint chunk publication](https://github.com/dmtrKovalenko/fff/blob/922bede74a531e66b469c73b628ac7ddfe278c63/crates/fff-core/src/index/bigram_filter.rs)
- [Unicode explanation of case folding versus case conversion](https://www.unicode.org/faq/casemap_charprop.html)

FFF is a design reference, not a compatible implementation to copy wholesale. Its printable-ASCII domain, one-file-per-slot layout and column-pruning/query contract differ from Objekte's. Preserve required license notices if source code is reused.

## Implementation report

Task `index-construction-data-path` completed 12 November 2026. Fixed baseline B was `ab616dc6aabb9b9fee4657deb6752a6512bec9e5` from remote `master`, including the selected construction merge and subsequent path/identity work. Integration branch: `integration/index-construction-data-path`; retained worktree: `/home/simon/source/objekte-integration-index-construction-data-path`. The original task specification was untracked at `docs/ai/tasks/objekte_index_construction_data_path_specification.md` (SHA-256 `97b60d8bc5351ba51343b9119b52ac1a1ee042ce210f5ec198ec1cc9d8e5ab07`) and was copied intact before this report was appended. The original checkout's `master` was not changed by this task.

### Implemented result and experiment history

The final code retains a borrowed scalar folded-stream path: ASCII `A..=Z` bytes are lowered directly, non-ASCII UTF-8 scalars use the locked full non-Turkic fold, and adjacent/skip-one gram history follows the same stream and segment boundaries as B. The final index and snapshot formats are unchanged. Agent 1's branch `task/index-construction-data-path/ascii-folding` contains SIMD candidate `d938fa5618` and scalar-only result `66c40c0edb`; the supervisor merged it at `ab463da9e6`. Native AVX2 was tested and rejected after complete gram-emission kernel trials; AArch64 NEON was written in the candidate but could not be compiled or run on the x86-64 host and was removed with AVX2 from final code.

Agent 2's `task/index-construction-data-path/compact-contributions` contains arena candidate `ee5e176416` and opt-in capacity profiling `78c290475a`. The candidate reduced process RSS but slowed full builds by 34–39% and used more measured global element capacity; it was not merged. Agent 3's `task/index-construction-data-path/parallel-admission` contains scoped merge candidate `f4e9c0013b` and a worker-count gate at `3cecb58840`. The supervisor merged it at `d53ee2363e`, adjusted the gate at `3db66fe9a9`, then removed its effective code at `4f51c45e22` after the combined Unicode regression. Both rejected branches and their evidence remain pushed. Independent evidence was archived at `382ba91251`; the complete [results report](../generated/index_construction_data_path_results.md) and [raw evidence](../../../bench/results/index-construction-data-path/2026-11-12-ab616dc6aa/README.md) were pushed at `f5a0ebcec0`.

### Measurements and validation

All ordinary comparisons used locked release builds, the existing `bench_index_construction` example and comparison driver, one warm-up per role, alternating fresh-process pairs, identical configured Musenalm data and worker settings, and separate target directories. Final B versus retained integration code measured paired median full-build changes of **−74.311 ms (−6.494%) ASCII** over five pairs and **−34.742 ms (−2.841%) Unicode** over three pairs. Only ASCII was extended from three to five pairs because its initial final comparison was inconclusive. Peak RSS differences were small in the final comparison; no memory saving is claimed for scalar folding. The report includes all trial values, RSS, binary hashes, corpus/schema fingerprints, separate opt-in component profiles, DHAT allocation counts, and baseline/compact contribution-capacity lower bounds. No independent gains were summed.

`cargo test --workspace --locked` passed: 816 passed, zero failed, 18 ignored across 21 groups. `cargo clippy --workspace --all-targets --locked` passed with established warnings. The repository formatting script was run but reports pre-existing formatting drift; baseline B also fails plain `cargo fmt --all -- --check`. Targeted changed-file rustfmt differences were confined to unchanged baseline assertions, and `git diff --check` passed. Focused bigram tests and admission tests passed on their experiment branches. Existing lifecycle, mutation, compaction, restore, and query tests passed in the workspace suite. Baseline and final snapshots restored under final code with `SnapshotMode::Required` and zero build parses for ASCII and Unicode. All 194 extended Musenalm queries matched between B and final index snapshots by exact count and ordered hit digest, including Q001, Q108, Q116, Q120, Q194 and hit status/diagnostic fields. Final index counters and snapshot payload sizes matched B. The AArch64 SIMD backend was not runtime-tested and is absent from the retained code.

There were no new dependencies, persistent-format changes, approved deviations, or unresolved in-scope correctness failures. The large-workspace test and relevant construction benchmarks were completed; no other application benchmark applies to this construction-only change. After the report commit was pushed, the original untracked task source was byte-checked against this committed specification and removed. All three subordinate experiment worktrees and the detached baseline helper worktree were checked for unpreserved files and removed normally; their pushed branches remain. Completed task memory was removed. The integration branch and worktree remain for the user's merge; no task commit was made to `master`.
