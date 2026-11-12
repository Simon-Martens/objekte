# Agent specification: merge selected index construction optimizations

Task identifier: `index-construction-selected-merge`  
Repository: `Simon-Martens/objekte`  
Decision date: 12 November 2026

## Objective and authorization

Merge the selected, already implemented index construction changes into `master`, after removing dispatch replenishment and validating the resulting combination.

The selection in this specification is final. Do not reopen the experiment programme or implement additional performance changes. Execution of this assigned task includes updating and pushing `master`; do not stop at an integration branch awaiting another merge approval. This explicitly overrides the ordinary repository rule that leaves completed tasks for the user's manual merge. Respect branch protection and preserve unrelated user work.

Use one supervising agent; this is a bounded integration task and does not require parallel implementation agents. Read the repository's applicable instructions and `docs/ai/README.md` before starting. Perform preparation, edits, tests and commits in a separate integration worktree. The original checkout stays on `master` and receives only the final validated merge.

## Selected changes

| Track | Decision | Reason |
| --- | --- | --- |
| M: bounded parallel metadata preparation, including the 64-path-per-worker window | Keep | Approximately 342 ms ASCII and 332 ms Unicode paired median build reduction in the recorded independent comparisons. This is the principal improvement. |
| E1: bounded input buffer reuse within each source batch | Keep | Wall time is approximately neutral, but sampled read calls fell from 17,804 to 4,672, allocation reuse is effective, and retention is bounded. |
| E4: direct JSON deserialization into existing logical values | Keep | Approximately 68 ms ASCII and 56 ms Unicode paired median build reduction; the 776-record allocation sample used 28.6% fewer allocated bytes. |
| A0: opt-in admission component and scheduling diagnostics | Keep | Shows where coordinator time goes. Keep disabled by default; it is diagnostic support, not an independently established speedup. |
| Removal of the unused admission wrapper | Keep | Applicable cleanup after the diagnostic admission entry point replaced its caller. |
| A1: replenish dispatch between individual ordered admissions | Remove | Approximately 22 ms ASCII and 14 ms Unicode paired median improvement costs approximately 8.7 MiB and 7.2 MiB additional peak RSS. The tradeoff is not worthwhile for this merge. |
| E2: retained worker-local bigram scratch | Exclude | Measured regression of approximately 47/34 ms and additional memory. Already absent from integration. |
| E3: inline field-selection storage | Exclude | No useful configured-build improvement; negligible measured allocation reduction. Already absent from integration. |

These measurements come from different paired comparisons. Do not add the individual savings or present them as the result of the final selected combination.

The earlier combined result, 2,107.469 ms ASCII and 2,255.591 ms Unicode, **included A1**. Measure the selection in this specification before reporting its final performance.

## Revisions and source evidence

Recorded baseline `master`:

```text
737cfb7745ff15915804553ab583af260ac0f2e4
```

Reviewed source branch: `integration/index-construction-optimization`  
Reviewed source head:

```text
7a84a368152ce018fc1e8cd0e297032bf8c0ea02
```

Its measured implementation revision, before the final documentation commits, is `609474ee42e26c568cd43f89893482e0a0886bbe`.

| Change | Integration commit |
| --- | --- |
| M metadata preparation | `9cacbe391a5125853a866148cf93d5ae33ac1f87` |
| M window size 64 | `41b3a68ae820eaca325975b62e4aad60bb4fb62c` |
| E1 input buffer reuse | `745841744782442c6dc6e346dc403f5d164c3014` |
| E4 direct logical deserialization | `ed38a27c47da2cdbde09e33f3ff8de3a5cb65cd6` |
| A0 admission diagnostics | `d91214ed8739ab3f574d8a71f98ac86d4396236c` |
| A1 dispatch replenishment — revert | `11dd9f5d5c0b5e24676fd7c68c12858506b762a8` |
| Unused admission wrapper cleanup | `609474ee42e26c568cd43f89893482e0a0886bbe` |

Read the existing evidence rather than reconstructing the earlier experiments:

- [Implementation results at the reviewed revision](https://github.com/Simon-Martens/objekte/blob/7a84a368152ce018fc1e8cd0e297032bf8c0ea02/docs/ai/generated/index_construction_optimization_results.md)
- [Benchmark archive and reproduction commands](https://github.com/Simon-Martens/objekte/blob/7a84a368152ce018fc1e8cd0e297032bf8c0ea02/bench/results/index-construction-optimization/2026-11-12-master-737cfb77/README.md)

Fetch the configured remote and verify these objects and their ancestry. Record the current remote `master` revision separately. If it has advanced, include its changes in the integration result and benchmark against that current revision. Do not silently include later, unreviewed optimization-branch changes.

## Integration procedure

1. Record the original checkout's branch and working/index state. Preserve all unrelated edits and task-document comments.
2. Create `integration/index-construction-selected-merge` from the reviewed source head, with worktree `/home/simon/source/objekte-integration-index-construction-selected-merge`. If already started, resume and inspect that worktree instead of overwriting it. Use sibling worktrees under `/home/simon/source`, never `/tmp`.
3. Revert only A1 using `git revert 11dd9f5d5c0b5e24676fd7c68c12858506b762a8`. Inspect the resulting patch. Resolve any conflict in the integration worktree.
4. Confirm that dispatch is no longer replenished after each individual ordered admission. Restore the preceding dispatch cadence while preserving A0's component timers and scheduling timestamps. Keep the unused-wrapper cleanup.
5. If remote `master` has advanced beyond the historical baseline, merge that current revision into the preparation branch. Resolve conflicts there. Preserve existing master behavior outside the selected changes.
6. Review the net implementation diff against current `master`: M, E1, E4, A0 and their relevant tests are retained; E2, E3 and A1 are absent from effective code.

Reverting A1 preserves the original experiment history and evidence. Its commit remaining in ancestry is acceptable; its dispatch behavior must be absent from the final code.

Push immediately after every commit, including the revert, any integration merge, tests and report commits. Stage explicit task-owned paths. Do not force-push, rewrite published history, reset user changes, automatically stash user work, or delete experimental branches.

## Required behavior

### Metadata and identity

Keep bounded Rayon preparation windows at 64 paths per resolved build worker, followed by ordered inventory insertion. Preserve discovery ordinal, deterministic file/record identity, collection membership, overlapping collection behavior and first-error selection.

Retain metadata validation, revision information, filesystem error handling, symlink behavior and non-UTF-8 path handling. Do not remove checks to improve the timing. Do not replace ordered insertion with concurrent ID allocation. Keep the existing worker-count policy and bounded work.

### Allocation and JSON semantics

Retain E1's per-batch buffer, bounded reservation/retention policy, complete reads to EOF and ownership transfer for malformed whole-file fallback. No record or asynchronous result may borrow bytes that the next file overwrites. Preserve oversize-buffer release.

Retain E4's construction-only direct parser into the existing logical value model. Preserve exact numbers, duplicate-key behavior, array semantics, Unicode handling, malformed-input diagnostics and fallback behavior. Keep the separate `JsonDecodedScalars` reader behavior.

The direct parser depends on serde_json's private arbitrary-precision number representation. Verify existing differential coverage against the old parser, including number boundaries/exponents and objects containing the literal `$serde_json::private::Number` key. Add a focused differential regression only where coverage is missing; mirror the existing parser's result or error rather than inventing new behavior. Keep dependencies locked.

### Batch admission

Keep A0 opt-in and disabled in ordinary builds. Preserve ordered admission, task/byte limits, error draining and prefetch behavior.

Do not change admission order, increase queue limits, split finalization stages, introduce io_uring, redesign posting/bigram merges, or add a new allocation framework in this task. The recorded long batch waits include coordinator service time and overlapping intervals; they are not recoverable elapsed time. The remaining substantial work is serial admission of bigram contributions and structured postings. Preserve the measurements for later investigation.

## Validation and simple measurements

Validate the final code, not the historical combination containing A1.

### Correctness

Run the repository's required formatting/check procedure on touched files, then:

```bash
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked
```

Use the existing metadata, buffer ownership/fallback, direct-parser and admission tests. Inspect their coverage against the invariants above; add tests only for a meaningful gap introduced or exposed by this integration. Do not duplicate already covered cases.

Normal Clippy previously passed with existing warnings. Record new diagnostics, fix warnings caused by this task where practical, and do not expand into unrelated lint cleanup. Do not invent a workspace-wide `-D warnings` gate.

Compare fresh-build query digests for Q001, Q108, Q116, Q120 and Q194 with the baseline, for both alphabets. Verify inventory/record counts, parse failures, field counts, relation edges and index-size counters. Existing figures for the unchanged configured corpus were 148,486 records, 300 posting fields, eight sort fields and 228,503 relation edges; use the actual baseline as the comparison authority.

Repeat the existing snapshot compatibility check using the selected build: load a baseline-produced snapshot and a selected-build snapshot in `SnapshotMode::Required`, with zero build parses and matching query digests. Reuse the archived verifier. Byte-identical snapshot hashes are not required.

### Performance

Use the existing `compare.py` driver and `bench_index_construction` example from the evidence archive; no new framework and no repeat of all experiment branches.

- Control: recorded current remote `master`, before this merge.
- Candidate: the exact selected code revision after the A1 revert and any master integration.
- Build both in release mode using `--locked` and separate target directories; record revisions and binary hashes.
- Use the same configured Musenalm corpus and schema for both binaries. Record fingerprints and resolved worker count; use the original 16-worker setting on the same host where available.
- One warm-up per binary, then three alternating control/candidate pairs for ASCII and three for Unicode. Fresh process per trial; profiling disabled; no concurrent compilation, tests or other benchmarks.
- Record raw `build_ns`, paired changes, medians, ranges and peak RSS. Report milliseconds and MiB.
- Run one separate profiled candidate build to confirm metadata concurrency, admission component output and interval accounting. Do not mix it into timing medians.

This is a short acceptance measurement. Do not rerun DHAT, syscall tracing, every worker count or every allocation experiment. Extend timing only if results show a material unexplained regression or instability. Do not claim that removing A1 necessarily saves its standalone measured memory delta; measure the combined result.

The selected combination should retain a clear construction improvement on the original fixture, primarily from M and E4. Investigate a material regression or correctness mismatch before merging. If the required corpus or build environment is unavailable, record the concrete blocker; do not invent results or silently waive validation.

## Reporting and final master update

Preserve the old benchmark archive unchanged. Its data remains evidence for the earlier combination.

Create `docs/ai/generated/index_construction_selected_merge_results.md` containing:

- Kept and excluded changes, source head, A1 revert commit and final tested implementation revision.
- Current master control revision, environment and exact commands.
- Final selected-combination measurements and memory results, clearly separated from historical A1-inclusive measurements.
- Correctness and snapshot results, residual limitations and any actual blockers.

Add a brief historical-status note to the old optimization report explaining that this merge specification supersedes its A1 retention decision; link the new report. Keep its original measurements and explanation intact.

Complete this specification under `docs/ai/done/` and append the required implementation report, preserving the user's original content. Follow repository rules for initially untracked or locally modified task documents. Commit and push the report and completed specification on the integration branch. These reports describe validation and readiness; do not claim that a master push has succeeded before it happens.

Then:

1. Fetch again and confirm that the validated integration result contains the current remote `master`. If master advanced, integrate it in the task worktree and repeat checks appropriate to the resulting code changes.
2. Record the final candidate SHA. Confirm that changes since the tested implementation are documentation-only, or validate any further code changes.
3. Confirm that the original master checkout can be updated without losing unrelated work or silently publishing unrelated local commits. Resolve ordinary integration conflicts in the task worktree; do not force through conflicting user edits.
4. Fast-forward the original checkout's `master` to the validated candidate using `git merge --ff-only <candidate-sha>`.
5. Push `master` to its configured remote and verify that remote master contains the validated candidate. A failed push is not completion; retain all work and report the reason. If branch protection requires a pull request, follow that mechanism and required checks without bypassing protection.
6. Report the actual resulting master SHA and push/merge outcome to the user, together with selected-combination timings and the completion report path.

Keep the integration branch/worktree and experiment evidence. No branch deletion is required. Clean only this completed task's temporary notes according to repository policy.

## Completion criteria

- M with the 64-path window, E1, E4, A0 and applicable cleanup are present.
- A1's behavior is removed; E2 and E3 remain excluded.
- Identity, validation, malformed-file fallback, exact-number behavior and snapshot/query semantics are preserved.
- Workspace validation and the short paired benchmark are recorded for the selected code.
- Completed task and results are committed and pushed.
- The selected result is merged into remote `master`, or an explicit repository/environment blocker is reported without claiming completion.

## Implementation report

Task `index-construction-selected-merge`, completed 12 November 2026 from
base `737cfb7745ff15915804553ab583af260ac0f2e4` on
`integration/index-construction-selected-merge`. Integration worktree:
`/home/simon/source/objekte-integration-index-construction-selected-merge`.

The reviewed optimization source was `7a84a368152ce018fc1e8cd0e297032bf8c0ea02`.
`5f03fb71a9f5dbcaf8914deac7ced83d3d3ec800` reverted A1's per-admission
dispatch replenishment; `e212e4ad5f2a691507540395d11947398b45ef36`
added differential parser cases for the reserved number key and numeric
boundaries. The selected code retains M with the 64-path worker window, E1,
E4, A0 and the unused-wrapper cleanup. E2 and E3 are absent. There were no
subtasks or contributing branches beyond the reviewed source integration
branch. The control `master` had not advanced when this combination was
prepared.

`cargo test --workspace --locked` passed with 811 passed, zero failed and 17
ignored. `cargo clippy --workspace --all-targets --locked` passed with
existing warnings. The touched Rust test was formatted, and its diff passed
`git diff --check`. The global spacing `--check` reports unrelated existing
differences; no broad formatting change was included. Fresh-build Q001,
Q108, Q116, Q120 and Q194 digests, inventory counts, zero parse failures,
posting/sort/relation counters and index-size counters matched remote
`master` for ASCII and Unicode. Both baseline and selected snapshots loaded
in `SnapshotMode::Required` with zero build parses and matching digests.

Using the archived `compare.py` driver with separate locked release targets,
one warm-up per role and three alternating pairs per alphabet, the selected
code's paired median build change was **−398.201 ms (−14.888%) ASCII** and
**−479.777 ms (−16.361%) Unicode** against the current remote `master`.
Median peak high-water RSS was 544.8→552.4 MiB ASCII and 567.0→556.0 MiB
Unicode. Exact commands, raw samples, environment, fingerprints and ranges
are in the [new results](../generated/index_construction_selected_merge_results.md)
and [acceptance evidence](../../../bench/results/index-construction-selected-merge/2026-11-12-master-737cfb77/README.md).
The A1-inclusive measurements in the older archive are preserved as
historical evidence. The three-pair memory samples do not isolate A1's
memory effect. There are no known correctness regressions or remaining
in-scope blockers. The selected integration branch and experiment branches
are retained. Master update and remote verification follow this report
commit; this report does not presume their success.
