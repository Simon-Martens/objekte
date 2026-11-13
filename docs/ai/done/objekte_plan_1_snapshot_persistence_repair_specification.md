# Objekte Plan 1: Snapshot persistence repair and memory reassessment

## Objective and execution order

Fix the three identified persistence defects in Plan 1 before any merge into master:

1. Repeated serialization of identical file metadata across the base inventory, lifecycle catalogue, and segments.
2. Serialization of lookup tables that can be reconstructed from authoritative records and file references.
3. Repository-sized file-offset arrays in empty base collections.

After these repairs, repeat snapshot-size and memory measurements, including saving and loading. Integrate the repairs forward through Plans 2 and 3 and evaluate the cumulative result. Preserve incremental availability and the new predicates/postings. Do not merge into master; the user will decide after reviewing the new measurements.

The separate desktop query/completion repair task remains independent. Preserve and integrate its changes if they arrive while this task is running.

## Integration ownership

Use the existing integration branches and worktrees:

| Plan | Branch | Worktree | Reviewed tip |
|---|---|---|---|
| 1 | `integration/incremental-index-availability` | `/home/simon/source/objekte-integration-incremental-index-availability` | `c54afc3eb1` |
| 2 | `integration/boolean-field-predicates` | `/home/simon/source/objekte-integration-boolean-field-predicates` | `7c8ce508d5` |
| 3 | `integration/indexed-field-structural-postings` | `/home/simon/source/objekte-integration-indexed-field-structural-postings` | `7a9dee532e` |

Original master reference: `7a6b96684e1fbb849eab7cc20185b942159ed189`.

1. Read repository instructions, fetch current branch heads, and inspect worktree status. Record actual starting commits; do not reset advanced branches or overwrite user changes.
2. Implement and validate the persistence repair on Plan 1. This explicit instruction to repair existing integration branches overrides the default convention of creating a new integration branch for each task.
3. Merge the repaired Plan 1 forward into Plan 2, then merge updated Plan 2 into Plan 3. Resolve conflicts against the current code and preserve predicate, structural-posting, desktop, and other intervening changes. Do not merge later-plan features backward into Plan 1 or duplicate the repair through unrelated cherry-picks.
4. Push after each implementation, integration, and report commit. Do not force-push or rewrite published history. Keep the original checkout on master and unchanged.
5. Any subordinate worktrees belong under `/home/simon/source`. One integrator owns the snapshot-format contract and forward integration; avoid concurrent edits to the same serializer/loader.

No approval pause is needed between the authorized repair, testing, forward integration, and measurements. Retain the integration worktrees for review. A successful test or benchmark does not authorize merging into master.

## Evidence and baseline

The published reports and raw samples show decimal bytes/MB:

| State | Completed snapshot bytes | Relevant memory evidence |
|---|---:|---|
| Original master | 129,139,458 | Plan 1 paired campaign: median peak after saving 543,768,576 bytes |
| Plan 1 / Plan 2 | 178,287,528 | Plan 1 paired campaign: median peak after saving 778,973,184 bytes |
| Plan 3 | 179,575,108 | Plan 3 campaign: median peak after construction 576,073,728; after saving 787,623,936 bytes |

Plan 1 contributes 49,148,070 additional snapshot bytes. Plan 3 contributes only another 1,287,580 bytes. Do not remove structural facts to conceal the Plan 1 cost.

In the Plan 1 archive, the base inventory section remains 21,502,142 bytes. Empty base collection sections total 4,216,586 bytes; their seven repository-sized offset arrays account for approximately 4.16 MB. The lifecycle section is 152,568,183 bytes. These are section measurements, not a complete attribution of the net increase: old and new collection representations differ.

Source references at the reviewed cumulative revision:

- `crates/core/src/storage.rs`: `EncodedFile`, `LifecycleCatalogFile`, `LifecycleCacheState::from_publication`, `EncodedCollectionRef`, and snapshot save/load.
- `crates/core/src/lifecycle_segment_codec.rs`: `EncodedSegmentFile`, `SegmentRef`, `OwnedSegment`, and segment encode/decode/validation.
- `crates/core/src/inventory.rs`: `FileRecordIndex`, `CollectionGeneration`, and `CollectionBuilder::finish`.
- `crates/core/src/repository_incremental.rs`: empty base creation and final publication.
- `crates/core/src/lifecycle.rs`: completion, publication ownership, and memory estimates.
- `crates/core/examples/bench_index_construction.rs`: construction and snapshot measurements.

Raw baseline archives are under `bench/results/incremental-index-availability/2026-11-13-final-paired-warm/` and `bench/results/indexed-field-structural-postings/construction-pairs/`.

## Phase 1: Establish byte attribution

Before changing the format, add narrow diagnostic accounting for actual encoded bytes. Attribute headers, file metadata, fingerprints/configuration identities, record data, derived mappings, empty-base structures, postings/bigrams/sorting/relations, and framing. Count each byte once; the breakdown must reconcile with file length.

Separate current metadata from historical metadata genuinely needed by immutable segments. Record how much is identical duplication and how much is semantically distinct. Account for per-file configuration identities that repeat the snapshot-wide configuration.

Keep diagnostics outside normal hot paths or disabled for comparable timing trials. Do not construct another full serialized snapshot merely to obtain the breakdown. Use existing measurements where adequate and add only the missing attribution.

## Phase 2: Repair the three representations

### 1. Canonical file metadata with explicit references

Replace repeated full metadata encodings with an authoritative representation and references. A canonical table plus versioned references, or an equivalent compact representation, is acceptable. The same file metadata version must not be serialized independently in the base inventory, catalogue, and each segment.

Required invariants:

- Preserve file IDs, retired IDs, current catalogue state, file incarnation/version tokens, source revisions, paths, collection membership, status, diagnostics, and fingerprint/configuration semantics.
- Immutable segments can require metadata from an earlier version. Do not substitute the latest catalogue entry for historical segment metadata, deduplicate by path alone, or assume equal modification times prove identical versions.
- Distinct metadata versions remain distinct when required for moves, replacements, diagnostics, or path-derived identifiers. Identical versions can share one serialized entry.
- Keep paths root-relative and preserve native non-UTF-8 path encoding where currently supported.
- Share common configuration identity where valid, with explicit exceptions when needed. Do not drop fingerprints or other lifecycle validation state to save bytes.
- Saving operates on one coherent pinned publication. It must not reread source content, compact/rebuild the entire index as a prerequisite, or mutate a reader's view.

Limit runtime changes to those required by the representation and correct reconstruction. Do not introduce a second complete resident repository to manufacture a smaller file.

### 2. Reconstruct derived lookup tables on load

Remove segment `id_lookup` and `file_index` from the persisted payload when their information is already present in authoritative record/file tables. Inspect other duplicated mappings only within this same local representation; do not turn this into a general index redesign.

- Rebuild each lookup once during loading from validated records or file references.
- Reuse the loaded record-ID allocation for lookup keys; do not deserialize a second copy of each ID string. The existing base loader already reconstructs `id_lookup` from records.
- Preserve duplicate-ID rejection, file ownership checks, slot bounds, ordering, and lookup behavior. Check counts and references before unchecked indexing or large allocation.
- Retain mappings that are not derivable without loss of semantics. Record the reason rather than guessing.
- Measure restore time and peak memory: a smaller file does not justify expensive repeated reconstruction during queries.

### 3. Compact empty base generations

Represent an empty base generation without an offsets vector proportional to every file in the repository for each collection.

- Preserve the distinction between an empty valid generation and an invalid/incomplete generation.
- Lookups for valid files must retain their existing empty-result semantics; out-of-domain references must still be rejected.
- Do not merely omit arrays on disk and recreate the same repository-sized zero arrays on load. Keep the empty representation compact in memory as well.
- Preserve nonempty collection mappings, mutations, compaction, validation, and restore behavior. Reuse the existing dense representation where it is appropriate.
- Construction must still publish catalogue metadata and complete segments at the existing readiness points. No delayed second parse or complete-copy consolidation is allowed.

### Format compatibility and integrity

Define one coherent format change. Update snapshot version handling so old serialized layouts cannot be decoded as the new layout. Existing incompatible-cache detection/rebuild behavior is acceptable; a migration framework is not required.

Preserve checksum validation, section/count limits, trailing-input rejection, atomic temporary-file publication, and failure/cancellation behavior. A failed save must leave the previous completed snapshot usable. Do not disable validation to improve load measurements.

Forward integration must preserve Plan 3's structural facts and version compatibility. Resolve overlapping version changes deliberately rather than mechanically accepting one branch's version number. Test each executable against its own new snapshots and verify unsupported old formats are rejected through the expected path.

## Phase 3: Correctness checks

Use deterministic fixtures covering:

1. Fresh completed incremental build, save, restore, and search with identical IDs, paths, result counts/digests, and completeness.
2. Multiple collections, overlapping file membership, multiple records in one file, and genuinely empty collections/base generations.
3. Add, delete, move, and replace operations before save and after restore; retired IDs are not reused and stale records do not reappear.
4. Metadata or path changes while immutable segments retain older contributions, including path-derived IDs and diagnostics where supported.
5. A held older view while a new publication is saved; both views remain coherent.
6. Raw fallback records, invalid-source diagnostics, fingerprints, and supported non-UTF-8 paths.
7. Duplicate IDs, invalid metadata references, invalid slots/counts, truncated/corrupt sections, and trailing bytes are rejected without publishing a partial index.
8. Initial construction cancellation/incomplete-save behavior and a failed save preserving the previous completed snapshot.
9. Format round trip for configured and inferred indexes. On Plan 3, explicitly verify `:?`, type predicates, presence/null/empty facts, and nested paths remain exact where previously exact.

Add assertions proving the three defects are removed: unique metadata-version encoding, absence of persisted derived lookups, and empty-generation storage independent of total repository file count. Returned query equality alone cannot prove the storage repairs.

Run focused tests after each change, then workspace tests, Clippy, and the existing query campaigns appropriate to each integration tip. Preserve result/source-open guarantees. Format touched code without sweeping inherited formatting drift. Smoke-test normal desktop/CLI save and reopen paths; a broad UI redesign or repeated unrelated UI campaign is unnecessary.

## Phase 4: Reassess the full memory cost

Measure after all three repairs and again after forward integration. Compare actual starting and repaired Plan 1, actual starting and repaired cumulative Plan 3, and a compatible original-master reference. Preserve distinct code revisions and formats in the measurements.

Use the same Musenalm corpus, configuration, alphabet, worker count, compiler, and cache conditions as the established protocol. Run seven alternating paired fresh-process trials for each before/after comparison; report medians and quartiles with raw samples. Reuse a baseline only when its environment and protocol are demonstrably comparable. Run Unicode correctness/storage checks as well as the principal ASCII comparison.

Measure three workloads separately:

| Workload | Required measurements |
|---|---|
| Construction | Catalogue readiness, first searchable publication, full completion, source reads/parses, peak RSS and post-build RSS |
| Build plus save | Whole-process peak RSS, post-save RSS, complete external save-call wall time, snapshot size and reconciled byte breakdown |
| Restore in a fresh process | Load wall time, peak RSS, post-load RSS, lookup reconstruction cost, and result correctness without a source-content parse pass |

A process high-water mark cannot be reset between phases. Use separate fresh-process workloads and clearly label cumulative peaks; do not subtract peaks and call the difference an allocation measurement. If phase-local live allocation data is available, report it separately with its method.

Time saving from before lifecycle-state capture and segment encoding until the full save call returns. The old inner save timer begins after some preparation and does not establish total persistence cost. Report payload estimates separately from total resident memory; the existing approximately 70 MB index estimate excludes substantial metadata and other allocations.

The writer currently retains encoded byte buffers for all segments before serializing the outer snapshot. After the three repairs, measure the remaining retained buffer bytes and their relationship to save-time peak RSS. Also check whether restore retains encoded payloads while decoded structures coexist. These are specifically required investigations in the memory reassessment.

Do not conceal remaining buffers behind a successful build-only RSS check. Do not automatically expand this task into an unrelated streaming-format rewrite: if the buffers remain a material cost, report that explicitly, identify the bounded follow-up needed, and leave the merge decision pending. The user's requested order is three representation repairs first, then a fresh assessment of memory cost.

## Acceptance and merge review

- All three representation defects are demonstrably removed, with lifecycle semantics and snapshot integrity preserved.
- Normal construction gains no additional source read/parse pass. Snapshot encode/decode does not read source content to rebuild indexes.
- No query correctness or source-open regression; preserve the original per-plan build, readiness, and query performance limits. Investigate repeated query regressions exceeding both 10% and 0.5 ms under the existing protocol.
- Snapshot reduction is explained by measured byte attribution. Explain residual growth against master; do not invent a percentage saving or remove features to hit one.
- Saving and loading memory/latency are reported alongside construction. There is no permission to treat an unspecified snapshot-memory limit as automatic acceptance of a large regression.
- Clearly distinguish implementation completion from merge readiness. Complete the authorized repairs and measurements without stopping for intermediate approval, then present the remaining costs for user review. Master remains unchanged regardless of the result.

## Commits and delivery

Use reviewable commits for attribution, the coordinated format/loader repairs, regression coverage, and reports. Keep intermediate published commits buildable; combine serializer/decoder changes where splitting them would break compatibility within a commit. Push every commit and integration merge.

- Completion record: `docs/ai/done/objekte_plan_1_snapshot_persistence_repair_specification.md`, preserving this task and appending the outcome.
- Plan 1 report: `docs/ai/generated/plan_1_snapshot_persistence_repair_results.md`.
- Final cumulative assessment: `docs/ai/generated/cumulative_snapshot_memory_reassessment.md` on Plan 3.
- Raw data: task-specific directories under the existing benchmark results hierarchy.

Preserve the previous specifications and reports as historical evidence. Add references to the new results rather than overwriting old measurements.

The final handoff must list pushed branch tips, worktrees, the final cumulative review tip, test results, before/after snapshot bytes, build/save/load memory and times, and any unexecuted checks or remaining material costs. Explicitly confirm that master was neither modified nor merged.

## Implementation report

- **Task:** `plan-1-snapshot-persistence-repair`; completed 2026-11-13.
- **Base and branch:** `c54afc3eb1cdc496734bfb30bcc9a3ea38988b78` on `integration/incremental-index-availability`; integration worktree `/home/simon/source/objekte-integration-incremental-index-availability` is retained for review.
- **Implementation commits:** `bf39483f7c` attributes every encoded byte; `2cb74685e1` canonicalizes metadata across base/catalogue/segments, retains unique historical versions, reconstructs segment lookup maps on load, compacts empty base offset tables, and changes snapshot versions to 27/28; `91326e6b5b` commits the paired measurements, report, and residual memory issue. No subordinate agent branch was used.
- **Forward integration:** Plan 1 was merged/pushed into Plan 2 at `9d562411a1`, then Plan 2 into cumulative Plan 3 at `23d38d5e3f`. The version conflict was resolved deliberately to 27/28. Cumulative tests and query validation were reported/pushed at `5e01d56d66`. Neither Plan 1 nor Plan 2 incorporated later-plan features backward. Master was neither changed nor merged.
- **Validation:** `cargo test --locked --workspace --quiet` passed on repaired Plan 1 (523 core library tests passed, 16 ignored) and cumulative Plan 3 (534 core library tests passed, 16 ignored). Plan 2's merged core library passed 528, 16 ignored. Focused metadata, invalid-reference/duplicate-ID, non-UTF-8 path, lifecycle mutation, failed-save, checksum/truncation, and old-version tests passed. `cargo clippy --locked --workspace --all-targets` exited 0 with inherited warnings; a strict `-D warnings` run failed on existing workspace lint locations. The cumulative 880-row query campaign preserved all statuses, normalized digests, counts, source opens and work counters; the only focused latency candidate did not repeat in five isolated pairs. Structural reference cases Q115–Q118/Q120 remained exact with zero source opens. Newly written caches restored; old Plan 3 version 26 was rejected as incompatible.
- **Performance:** Seven alternating paired fresh-process profile trials on Plan 1 measured snapshot size 178,287,528→131,422,504 bytes, median full save call 380.92→293.98 ms, median save peak RSS 775.18→715.42 MB, median fresh restore 923.97→733.36 ms, and median restore peak RSS 492.11→405.38 MB. Construction reads/parses remained 148,486 each; fresh restore made zero source reads/parses. A second unprofiled seven-pair campaign and Unicode correctness/storage check corroborated the result. Seven cumulative Plan 3 pairs measured 179,575,108→132,710,084 bytes, full save call 389.39→306.80 ms, save peak 783.03→720.42 MB, fresh restore 956.23→757.91 ms, and restore peak 493.39→406.60 MB. The original master reference snapshot remains 129,139,458 bytes; it uses a different nonincremental path. Commands, environment, inclusive quartiles, raw samples and manifests are in the linked reports and benchmark archives.
- **Remaining cost:** The repaired Plan 3 writer still retains 104,163,001 bytes of encoded segment payloads during save; restore temporarily keeps encoded and decoded segments together. This is documented in `docs/ai/issues/perf/lifecycle_snapshot_segment_buffer_peak.md` for user merge review. A streaming-format rewrite was outside this task's authorized three-representation repair and was not performed.
- **Reports:** `docs/ai/generated/plan_1_snapshot_persistence_repair_results.md` and `docs/ai/generated/cumulative_snapshot_memory_reassessment.md`. Raw data are under `bench/results/incremental-index-availability/2026-11-13-snapshot-repair-{profile-pairs,pairs}/` and `bench/results/indexed-field-structural-postings/2026-11-13-snapshot-repair-profile-pairs/`.
- **Cleanup:** Integration worktrees remain. The detached measurement worktrees are task-owned and are to be removed after verifying no unpreserved files; the original checkout's obsolete untracked task source is removed only after this completed document is confirmed on the remote. The separate desktop repair remains independently authorized and in progress on Plan 3.
