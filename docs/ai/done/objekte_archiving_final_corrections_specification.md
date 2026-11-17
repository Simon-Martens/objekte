# Objekte: archiving final corrections specification

Date: 2026-11-17

Repository: Simon-Martens/objekte

Reviewed master: a298e42e941fc9cfc6756e44e5bef835c84eb1fc

Task identifier: archiving-final-corrections

## 1. Assignment and scope

Implement the four corrections below on the current master branch, continuing the explicitly authorized master repair task. Deliver working code, focused regression tests, and a short completion report. This is an implementation assignment, not another investigation-only report.

The reference revision identifies the reviewed implementation. Master may have advanced and may contain concurrent work. Read the current repository instructions, inspect the current implementations, and verify each finding before editing. If a correction is already present, preserve it and establish its acceptance evidence. Do not reset master, revert previous repairs, overwrite unrelated changes, or rebuild the entire archiving design.

The review identified two behavioral regressions and two implementation gaps:

| Requirement | Classification | Required outcome |
| --- | --- | --- |
| R01 | Query correctness | Text metadata compared with null follows the existing comparison semantics. |
| R02 | Lifecycle correctness | A completed quiesce call cannot expose an unfinished coordinator transition to resume. |
| R03 | Validation gap | Converted segment rows and lookup tables undergo their cross-component semantic check before installation. |
| R04 | Serialization implementation gap | Path serialization uses supported archive construction without handwritten replicas of enum layouts. |

Retain the completed improvements: borrowed metadata evaluation, shared immutable payloads, preserved cache owners, borrowed catalog serialization, shared catalog-shard conversion, one bounded conversion schedule, atomic publication, and retained outcomes for earlier drain operations.

Production report-field accounting, ownership-count metrics, retirement timers, and the former A01–A42 coverage ledger remain out of scope. Disk persistence, new storage formats, new APIs, general optimization, and the cold Q001 investigation are also outside this task.

## 2. Establish the actual starting point

Before editing:

1. Read AGENTS.md and the applicable docs/ai instructions.
2. Record the current master revision and working-tree status. Inspect divergence from the reviewed revision as needed; do not assume the listed line numbers remain current.
3. Locate the functions identified below and the existing tests that exercise them.
4. Identify unrelated changes and use a single writer for task-owned files on master. Do not stage or commit someone else's work.
5. Confirm the existing package names and pinned rkyv version from the checkout.

Existing successful repairs must remain intact. If concurrent changes already implement a requirement, use their implementation and add only missing regression coverage. Describe such cases accurately in the final report.

## 3. R01 — preserve null semantics in borrowed metadata comparisons

### Observed defect

In reference.rs, compare_metadata_literal handles MetadataOperand::Text by first extracting a text literal. A null literal fails that extraction and returns false, including for inequality.

The binder accepts null with equality and inequality. The existing compare_scalar rule returns true for a non-null value compared with null using inequality. Consequently, expressions such as the following incorrectly reject records in the reviewed implementation:

    @record.id != null
    @file.path != null
    @collection.name != null
    @repository.root != null

Both the reference evaluator and segment execution use the new helper. Native/archive equality alone therefore cannot establish correctness: both implementations can produce the same incorrect result.

Review reference: [reference.rs](https://github.com/Simon-Martens/objekte/blob/a298e42e941fc9cfc6756e44e5bef835c84eb1fc/crates/core/src/reference.rs#L1894).

### Required implementation

Handle a null literal before the text-only literal conversion, or use a common scalar null-comparison helper shared by owned and borrowed evaluation.

Preserve this truth table:

| Left operand | Literal | Equality | Inequality |
| --- | --- | --- | --- |
| Non-null metadata | null | false | true |
| Null metadata | null | true | false |

Preserve the established treatment of a null left operand with a non-null literal and field-to-field comparisons. Do not import different SQL null semantics. Keep ordered and containment comparisons with null rejected by the binder.

The correction must retain borrowing. Do not allocate Value::String, RecordId, RecordEntry, or FileEntry objects solely to route ordinary metadata comparisons through the old evaluator. Existing scalar enum values or a small shared helper are suitable.

### Required regression evidence

Use an existing small fixture and explicit expected IDs or counts.

- Test equality and inequality with null for record ID, file path, collection name, and repository root.
- Exercise direct/reference evaluation and public search in native and archived representations.
- Include a deterministic optional timestamp case: missing file-created metadata and present file-created metadata. Construct metadata in a fixture if filesystem timestamp behavior is unreliable.
- Retain a representative valid metadata field-to-field comparison and an invalid null ordering/containment binding case.
- If a supported text transformation takes this fast path, include one null comparison through that transformation.
- Verify nonzero and zero-result cases.

Assert the expected results independently before comparing execution modes. Extend the existing metadata tests; do not create a second query evaluator as the test oracle.

Acceptance: the previously incorrect inequality expressions select all applicable non-null records, equality selects none, optional null metadata retains its established behavior, and ordinary candidate evaluation remains borrowed.

## 4. R02 — commit quiescence state before notifying completion

### Observed defect

Index::quiesce currently completes the selected CloseOperation before changing CloseCoordination from Draining to Quiesced. Completing the operation wakes callers waiting on it.

A valid interleaving is:

1. Caller A finishes the drain and completes the operation.
2. Waiting caller B returns successfully from quiesce.
3. Before A updates CloseCoordination, B calls resume_after_quiesce.
4. Resume still sees Draining and returns Cancelled.

The retained operation fixes the original lost-outcome race, but the ordering leaves this new completion/resume window.

Review reference: [lifecycle.rs](https://github.com/Simon-Martens/objekte/blob/a298e42e941fc9cfc6756e44e5bef835c84eb1fc/crates/core/src/lifecycle.rs#L3153).

### Required implementation

Make coordinator completion and operation-result publication one correctly ordered transition. Before a caller can observe the operation's completed result, the coordinator must have committed the corresponding Quiesced state.

The straightforward approach is to acquire the coordinator lock after draining, confirm that it still selects this operation, change its state to Quiesced, publish the operation result and notify waiters, then release the coordinator lock. Follow the actual lock-order rules in the current code. Completing the operation may take its short result mutex; it must not wait for a worker or another operation.

Required invariants:

- One leader performs each drain; followers retain the selected operation.
- Every result, including an error, remains attached to its own operation.
- Resume followed by drain B cannot change the result received by a delayed drain-A caller.
- No coordinator/publication/transition lock remains held while joining workers or waiting for a drain result.
- Preserve the existing publication-lock order and the release of the transition gate before worker cancellation/join.
- Resume after a completed quiesce succeeds when no independent caller has started a newer drain. This does not require resume to override a legitimately active newer operation.
- Unexpected coordinator states are not silently reported as successful completion.

Do not implement polling, sleep-and-retry, discarded errors, or a global last-result slot.

### Required regression evidence

Extend the deterministic gate-based lifecycle tests. Exercise the exact transition between performing drain work, exposing the result, and updating the coordinator.

Arrange for a follower to observe completion and attempt resume before the leader's outer quiesce call returns. With no new drain started, resume must succeed. A test hook may pause the leader after the completion transition and after releasing coordinator locks. Use test-only transition instrumentation where necessary to establish that notification cannot precede the state change; do not rely on a scheduler race happening by chance.

Retain or extend the existing tests for:

- Multiple followers selecting drain A.
- A delayed follower surviving resume and drain B.
- Drain A returning an injected error.
- Concurrent reconciliation preparation and a paused archive completing without deadlock.

Use bounded test waits for diagnosis and barriers/channels for ordering. Increasing a timeout does not repair a race.

Acceptance: observing completed quiescence implies the corresponding state transition is committed, and all previous per-operation retention and deadlock tests remain valid.

## 5. R03 — restore semantic validation of converted segment components

### Observed gap

IndexedCollectionSegment::archived_candidate converts file/record rows, file lookup, and record-ID lookup but no longer calls validate_segment_components before returning the replacement.

The conversion still checks archive bytes and performs some local row checks. PublishedSegment::checked checks descriptor, liveness, and completion conditions. Neither substitutes for checking the relationships among the newly converted rows and lookup tables.

This is a removed validation boundary; the review did not establish existing data corruption.

Review references: [candidate conversion](https://github.com/Simon-Martens/objekte/blob/a298e42e941fc9cfc6756e44e5bef835c84eb1fc/crates/core/src/native_segment.rs#L500) and [component validator](https://github.com/Simon-Martens/objekte/blob/a298e42e941fc9cfc6756e44e5bef835c84eb1fc/crates/core/src/segment_archive.rs#L1785).

### Required implementation

Restore validate_segment_components, or its current equivalent, after the converted components exist and before the candidate is accepted. Validate the converted objects, not merely their native sources.

Preserve the established invariants, including:

- File and record rows share the intended archive owner.
- File lookup coverage and ordinal mappings agree with the file rows.
- Collection membership and file ownership agree.
- Every record has exactly one valid owner.
- Record slots and byte ranges are valid.
- Record IDs and ID lookup entries agree, with valid coverage and uniqueness.

Use the existing borrowed archived traversal. At the reviewed revision, the validator reads archive roots directly and does not need compatibility row reconstruction. Retain that property.

Run the check once per converted segment result, inside the bounded preparation job where appropriate, before publication. Do not add redundant whole-publication scans for every wrapper sharing the same validated result. Keep unchanged already archived owners on the established reuse path.

Propagate validation errors through the existing transactional failure path. Preserve the installed publication and pinned readers; discard unpublished candidates and allow retry.

### Required regression evidence

Existing stage-error injection is insufficient by itself: it can succeed without executing the real semantic check.

Construct small, byte-valid but semantically inconsistent candidate components using test-only facilities. Prefer a valid lookup from another small fixture or a deliberately wrong mapping with valid archived encoding. Do not corrupt memory or use unchecked archived roots.

Require rejection for representative file-lookup and ID-lookup mismatches. Ensure at least one test exercises the actual conversion acceptance boundary, so removing the validation call would make that test fail. A direct unit test of validate_segment_components alone does not prove the caller uses it.

For an archive attempt rejected at this boundary, verify unchanged publication identity/revision and query answers, a still-usable pinned view, and successful retry after removing the injected inconsistency. Reuse the existing atomicity fixtures rather than duplicating a broad mutation suite.

Acceptance: newly serialized components are cross-validated before installation, and this check neither reconstructs compatibility rows nor serializes all workers behind one lock.

## 6. R04 — replace handwritten enum layout replication

### Observed gap and risk

archived_path.rs defines BorrowedStoredPathTag, BorrowedPlainVariant, and BorrowedEscapedVariant to mirror the generated StoredPath archive enum. Its resolver uses cast_unchecked to write through those substitute layouts.

The earlier specification explicitly prohibited this approach. The review did not prove undefined behavior on the current build. The problem is the handwritten dependency on generated enum layout in a correctness-sensitive serializer.

Review reference: [archived_path.rs](https://github.com/Simon-Martens/objekte/blob/a298e42e941fc9cfc6756e44e5bef835c84eb1fc/crates/core/src/archived_path.rs#L45).

### Required implementation

Remove the handwritten variant layouts, discriminant-writing code, and casts to their substitute types. Preserve the existing archived path type, lossless path semantics, checked validation, and borrowed production serialization.

Prefer a derived borrowed source representation that targets the existing archived type. The rkyv Archive derive documents an “as” attribute for using an existing archived type, and AsOwned supports archiving borrowed Cow string/slice data into the corresponding owned archive representation. Verify the exact syntax, generated code, and supported combinations in the version pinned by this repository before selecting the implementation.

A concrete design to evaluate is an alternate source enum with the same variants and field names, borrowed Cow string/slice fields, appropriate wrappers, and a derive targeting ArchivedStoredPath. Ensure matching discriminants and archived field types. Do not treat this design sketch as already compiled code.

If that combination is unsuitable in the pinned version, use another supported generated adapter or serializer construction method. Do not upgrade rkyv, change the archive format, or recreate the same handwritten layout under new names merely to complete this task.

Useful primary references:

- [Archive derive attributes](https://docs.rs/rkyv/0.8.18/rkyv/derive.Archive.html)
- [AsOwned string and slice support](https://docs.rs/rkyv/0.8.18/rkyv/with/struct.AsOwned.html)

These references describe 0.8.18; the checked-out dependency and its source govern implementation.

### Ownership and platform requirements

- Ordinary UTF-8 paths must not acquire a new String copy solely for serialization or resolution.
- On Unix, borrow native path bytes for escaped/non-UTF-8 paths.
- Retain cached query text safely through serialization and resolution. If an accessor returns an Arc, retain its owner; never return a reference into a dropped temporary.
- Windows native code units must remain lossless. Use an established iterator adapter or reusable serializer/job-local scratch if encoding requires it. Keep any necessary conversion bounded; do not reconstruct every full owned row or repeatedly allocate path buffers.
- Preserve the distinction between plain paths and escaped query representations. A valid UTF-8 path can still require escaping.
- Small, documented unsafe projections onto actual archived fields may remain where the supported API requires them. Handwritten replicas of enum layouts and unchecked archive access may not.

### Required regression evidence

Reuse the owned adapter as a test oracle, not as the production implementation.

Run checked round trips for empty and ordinary paths, short and long Unicode strings, escape-marker ambiguity, and non-UTF-8 native paths on Unix. Include paths around any inline/outline string boundary already covered by the archive tests.

Exercise both catalog and segment serialization, with existing diagnostic, optional-field, malformed/fallback, and zero-record fixtures. Assert exact native-path recovery and query representation equality. Semantic equality is required; byte-for-byte equality is required only where the format contract already demands it.

Retain malformed-archive rejection tests. Review the executed adapter path to confirm that the owned row/path constructors are not reintroduced. Existing lightweight instrumentation is sufficient; a new allocator profiler is not required.

Run supported Windows checks when the target/toolchain is already available. State plainly if Windows execution was unavailable; do not download or configure a new platform environment for this correction.

Acceptance: the production adapter contains no handwritten enum-layout casts, retains borrowing on ordinary paths, and produces validated archives read correctly through the existing archived type.

## 7. Verification and performance budget

Keep verification proportional to these changes.

1. Run the relevant focused tests while implementing each correction.
2. After the final code changes, run the core test suite once, format checks, and workspace compilation for affected consumers, normally cargo check --workspace --all-targets.
3. Include relevant existing lifecycle/archive integration tests if the core command does not include them.
4. Reuse successful checks for unchanged code. Documentation-only edits do not require another test run. A later code change requires rerunning the affected checks.

Use the current package names; the reviewed core package is objekte-search-core. Do not weaken assertions, disable validation, or mark unrelated requirements passed to obtain a clean report.

R01 and R02 need no performance campaign. Because R03 restores work during archive preparation and R04 changes serialization, allow one representative build-then-archive smoke run after both are complete, using the existing executable and dataset if available. Use the established ASCII/32-worker configuration. Record native build and explicit archive separately. Use small fixture queries with explicit expected results to verify semantics.

There is no requirement for a full query suite, worker/alphabet matrix, historical rebuild, allocation profile, repeated cold/warm series, or new benchmark harness. Reuse compatible measurements already produced for the final code. If a concrete material regression appears, repeat only the affected operation, at most three times.

The prior report's approximately 887 ms build and 98 ms archive times are single-sample context, not hard thresholds. Restoring a required validation pass can add legitimate work; report the measured cost instead of removing validation again.

Do not investigate or optimize the previously observed first archived Q001 latency as part of this task. If the representative dataset is unavailable, use the existing small fixture and identify that limitation.

## 8. Delivery and acceptance

Use small commits that group each correction with its regression coverage. Complete the repository's task-document/report procedure, preserve concurrent work, and commit/push the authorized changes to master without force-pushing.

The completion report should contain:

- Starting and final master revisions.
- One concise result for R01–R04, naming the relevant tests.
- Checks actually run and their outcomes.
- The single archive smoke result, if run, and any platform limitation.
- Any unresolved requirement, distinguished from an already verified correction.

Do not claim the task is complete merely because native and archived results agree, an injected stage returns an error, or an archive passes byte validation. The explicit semantic expectations, lifecycle ordering, real component checks, and supported path construction are the acceptance criteria.

Deliver the implementation and evidence. Do not replace unfinished work with another proposed repair plan.

## Implementation report

- Task: `archiving-final-corrections`; completed 2026-11-17.
- Starting master: `a298e42e941fc9cfc6756e44e5bef835c84eb1fc`.
- Final implementation revision before this report: `5850fbfe7e4e204e0741a2f9b24643a2714b8dd1`.
- Integration branch/worktree: `master` in `/home/simon/source/objekte`, as authorized by the task.
- Implementation commits: `5b0015a06d` (R01), `32f2c726a8` (R02), `8af37d62d1` (R03), and `5850fbfe7e` (R04).

### Corrections

- R01: Borrowed metadata comparisons now handle a null literal before text extraction while retaining borrowed operands. `scalar_metadata_comparisons_preserve_null_and_timestamp_rules` and `metadata_null_literals_match_across_reference_native_and_archive` verify null equality/inequality for ID, path, collection, repository root, transformed ID, optional created metadata, field-to-field comparison, invalid ordering/containment binding, explicit IDs/counts, reference execution, and native/archive public search.
- R02: Quiescence commits `Quiesced` under the coordinator lock before publishing and notifying the selected operation result. `quiescence_commits_coordinator_state_before_followers_resume` pauses after the transition, lets a follower return and resume before the leader's outer call returns, and retains the existing multi-follower/error/deadlock suite.
- R03: Converted segment rows and lookup tables are cross-validated before candidate acceptance. `converted_segment_lookup_mismatches_fail_before_publication_and_retry` injects byte-valid empty file and ID lookup tables, verifies rejection at the conversion boundary, unchanged status/revision and pinned queries, and successful retry without reconstruction of compatibility rows.
- R04: The destination path adapter now uses a derived borrowed source enum with `#[rkyv(as = rkyv::Archived<StoredPath>)]`; no handwritten enum tag/variant replicas or casts remain. `borrowed_path_adapter_round_trips_plain_unicode_and_escaped_paths` covers empty, ordinary, long Unicode, escape markers, and Unix non-UTF-8 paths; existing catalog and segment borrowed-row round trips and malformed/fallback archive tests also pass.

### Validation

- `cargo test -p objekte-search-core`: passed 613 library tests, 30 integration tests, and 1 doc test; 17 ignored; 0 failures.
- `cargo check --workspace --all-targets`: passed for core, CLI, and desktop.
- `rustfmt --edition 2024 --check` on every changed Rust file: passed.
- Repository-wide `cargo fmt --all -- --check` remains non-clean because of extensive pre-existing formatting differences in unrelated CLI/desktop/examples files; no task file was reported by the changed-file check.
- Windows execution was unavailable in this environment. Unix escaped/non-UTF-8 paths were exercised; Windows native-unit serialization uses the existing `encode_wide` iterator path and was compilation-reviewed but not cross-compiled.

### Archive smoke

Command: existing `bench_musenalm` executable, Musenalm, ASCII, 32 query workers, 32 archive workers, verified cache disabled, Q001/Q034/Q119/Q149, one sample.

- Native open/build: 837.670 ms.
- Explicit archive: 103.436 ms; preparation 35.253 ms; peak 32 conversion jobs; 149,111,376 archive bytes.
- All four queries completed successfully with the expected match/page counts.
- Result digest: `71f7ff66bd31e3a6b4b7222fa2732c4d5a9ba5ad491d431d929f59fe3b346951`.

### Remaining limitations

No blocker or unresolved in-scope requirement remains. Windows runtime validation and the unrelated repository-wide formatting baseline are identified above. The prior cold Q001 investigation and excluded reporting/accounting work were not expanded.
