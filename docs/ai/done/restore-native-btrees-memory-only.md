# Restore native B-trees and remove all disk-index persistence

Date: 2026-11-14  
Task identifier: `restore-native-btrees-memory-only`  
Status: hardened handoff specification; no implementation or revert has been performed. Execution requires the user to switch to an implementation-capable agent and authorize execution.  
Repository: `/home/simon/source/objekte`

## Handoff package — read first

This document is the scope and carry-forward specification. Hand over **all three files**:

1. This main plan.
2. [Execution handoff](restore-native-btrees-handoff.md): resolved design decisions, per-file edits, interface transformations, test migrations, named regression gates and compilation-coherent checkpoints.
3. [Complete commit ledger](restore-native-btrees-commit-ledger.md): a disposition for each of the 236 commits, including all 39 merges and both nonempty merge resolutions.

The handoff's explicit decisions and checkpoint order take precedence over earlier broad implementation language below. It settles configuration identity, source exclusions, readiness/errors, native maintenance estimates, cancellation, protocol versioning and storage-backed tests.

Suggested instruction for the implementing agent:

> Implement `/home/simon/.opencode/plan/restore-native-btrees-memory-only.md` together with its two linked companion documents. Follow the handoff checkpoints C0–C5 and repository workflow. Preserve every K01–K29 item, account for any changes after the audited head, and report actual verification evidence. Restore native B-trees, remove all index persistence and frontend controls, and defer B+trees. Do not restore mixed files wholesale or weaken tests to make the rollback pass.

This is an execution prompt for the user's next implementation session, not authorization to implement during this planning session.

## 1. Agreed outcome

1. Restore the native in-memory index representation used before hybrid serialization.
2. Delete every disk snapshot/index persistence API and implementation, including both the old snapshot stack and the newer managed-generation stack.
3. Remove the corresponding functionality from the CLI, CLI/TUI, and native GUI.
4. Preserve or reintroduce all subsequent changes unrelated to persistence, including the completed prepared-query binding-cache deletion.
5. Introduce B+trees in a later, separate task.

The finished application builds its index from source files into RAM on each opening and maintains it there. It has no index save/load, import/export, automatic synchronization, shutdown save, recovery, disk cache, mmap-backed index, or temporary index spill.

Source documents, schemas, saved queries, preferences, user-requested result exports, and benchmark reports retain their ordinary filesystem behavior. In-process immutable query views and publication revisions remain part of the live index.

This plan supersedes `memory-only-bplus-index.md` and the earlier TOML/disk-storage plans.

## 2. Exact rollback reference and audited history

### Native implementation reference

`B = da8fde68472b8646a63a05b3b3654c58de877301`

Commit: **Gate detailed profiling and publish standard timing milestones**.

Inspection of that revision confirms:

| Structure | Native representation to restore |
| --- | --- |
| Scalar value postings | `BTreeMap<ScalarKey, RoaringBitmap>` |
| Length postings | `BTreeMap<Number, RoaringBitmap>` |
| Structural facts | `BTreeMap<StructuralKey, RoaringBitmap>` |
| Normal and skip bigrams | Separate `HashMap<u64, RoaringBitmap>` maps |
| Records, file memberships, paths and identities | Native owned metadata and the existing maps/vectors |

Restore these structures and their direct lookup/range operations. Retain the existing efficient bigram construction buffers; the completed gram index should again own native maps and bitmaps.

The original task is `integration/hybrid-index-storage`. Its specification at `docs/ai/done/objekte_hybrid_index_storage_implementation_plan.md:206` explicitly replaces the allocated `BTreeMap` with encoded leaf blocks. Representative transition commits are:

- `083fc1e6b0df`: immutable segment and ordered-block primitives.
- `54bcf9bf85d5`: serialized typed-posting adapter.
- `a0c9bddbd6cf`: packed normal/skip gram dictionaries.
- `e4450ba89f1f`: install serialized dictionaries and release builder maps.
- `b0c0003a3f95`: route queries through serialized dictionaries.
- `8b166cc7463c`: serialize memory-only lifecycle builds as well.
- `23c4b1a5886a`: seal static-build shards before publication.

### Current preservation reference

`H = a0a0691897c6ff24985fc4db9ae51f5d77a89899`

Commit: **Complete explicit index persistence task report**. The inspected checkout is clean `master...origin/master`.

The full reachable range `B..H` contains **236 commits: 197 non-merge commits and 39 merges**. Planning examined that full log, task completion records, aggregate changed-path diffs, and substantive patches for the restoration/preservation areas below. The inventory includes side-branch history, not just first-parent commits.

The main later task groups are:

1. Hybrid storage implementation.
2. Hybrid storage repair.
3. Allocation and query performance follow-up.
4. Generation storage, validation stamps and posting proofs.
5. Query scanner plans and hot-path optimization.
6. Frontend storage parity.
7. Core compile-warning cleanup.
8. Direct Snapshot Access.
9. Prepared-query binding-cache removal.
10. Legacy snapshot-stack removal.
11. Explicit index persistence.

Earlier incremental availability, desktop query/completion repairs, construction work and lifecycle repairs already exist in `B`. For example, desktop repair completion `a5fdd933255a` and snapshot/admission baseline merge `9c6c9e1af37c` are ancestors of `B`; restoring `B` does not require replaying those tasks.

The separately mentioned TOML implementation is not present in the inspected history. At execution, record the actual starting revision and examine any additions since `H`, including the intended TOML branch if it has become part of that starting revision.

## 3. Rollback method

Create `integration/restore-native-btrees-memory-only` from the actual current integration base, in sibling worktree:

`/home/simon/source/objekte-integration-restore-native-btrees-memory-only`

Apply the rollback as new, reviewable commits:

1. Restore the native representation and its consumers from `B`.
2. Apply the persistence deletions and carry-forward changes described below.
3. Validate the final state against both `B` and the actual starting revision.

Use `B` as the source for the old implementation, and `H`/the execution base as the source for later retained behavior. This is a functional rollback followed by selective reapplication; existing published history remains intact.

Do not run a blanket reverse of `B..H`: it includes merges, subsequent removals, historical reports and mixed-purpose commits. Do not reset or force-push master, replace the entire repository tree, or overwrite user work. Restore explicit code areas through reviewed patches and adapt the retained changes to native structures.

Existing task reports and archived measurements remain historical records. Update current documentation to the memory-only design.

Follow `docs/ai/README.md` for task memory, integration worktree, explicit staging, immediate pushes, validation and completion reporting. This plan authorizes planning only; implementation starts through a separately authorized execution session.

## 4. Carry-forward inventory

Every row below is part of the intended final result. For mixed commits, carry forward their applicable changes and tests without restoring disk-oriented adapters.

### 4.1 Completed removals and construction improvements

| ID | Source commits | Preserve/reapply |
| --- | --- | --- |
| K01 | `1e22afd17bec` | Complete prepared-query binding-cache removal: fresh binding, deleted `PreparedCache`, repository mutex/ownership, `prepared_cache_bytes`, hit counters, retained-size accounting, cache tests and benchmark options. |
| K02 | `edb1e753db7b` | Feature-gated binding timing and fresh-binding verification. Preserve the historical measurement archive; adapt runnable comparisons to memory-only construction. |
| K03 | `e5d6f250bdc7` | Configuration identity extraction into `configuration.rs`. Live prepared updates, source fingerprints and desktop session preferences still use configuration identity; it must not depend on a deleted snapshot module. |
| K04 | `da608b2799a6`, with earlier removals `5342a54a08c6`, `527165108ae8`, `d6fe6083e5ff`, `196660e8b090` | Keep the legacy snapshot/lifecycle-cache stack deleted, including API/options, automatic-save behavior, frontend state and obsolete benchmarks. Adapt migrated tests to native memory construction rather than managed storage. |
| K05 | `bfca5ba9d205` | Preserve the construction improvement hidden in the first hybrid commit: borrow `BatchContribution::logical_records()` instead of cloning its bitmap; separately account for inference copying/merging in incremental admission. |
| K06 | `a135638933f6`, relevant benchmark provenance fixes | Preserve accurate construction/publication timing and binary/fixture provenance. Keep preparation/install measurements where useful; remove serialized-sealing measurements and disk-mode comparisons. |
| K07 | `6ddde6e7e202` | Preserve applicable dead-code/test-only cleanup in surviving native code, notably bigram/relation projection helpers. Remove newly dead code after restoration and retain clean compiler checks. |

**Binding-cache boundary:** preserve verified-result, inferred-order and relation-target memory caches with their existing ownership/budgets. They are different features. The decoded-posting cache disappears with the serialized-posting backend.

**Configuration boundary:** preserve the current hash byte-for-byte for unchanged inputs, including its fixed domain/semantic identifiers, because live prepared updates and existing GUI source-check preferences consume it. Delete actual disk codec/manifest versions with their implementations. The exact decision is handoff D1; do not redesign identity in this task.

### 4.2 Query execution and diagnostics

| ID | Source commits | Preserve/reapply |
| --- | --- | --- |
| K08 | `5020aa328d15` | Structural postings for every declared field, including value-unindexed fields; retain explicit `values_indexed`/coverage semantics and auto-index exclusion rules. Reapply only native field construction/validation changes, not persisted binding flags. |
| K09 | `0a80e830a4ea` | Exact routing for declared array-step state predicates, including schema checks that every traversed `[*]` is a declared array. Keep positional and unsupported cases on the evaluator. |
| K10 | `a09368464796` | Proven scalar self-comparison optimization and EXPLAIN reasons. Preserve all eligibility guards and raw-presence semantics, including present empty selections. |
| K11 | `9835f0c4e7b9`, `3b959f10555a` | Inline single-file ranges; group contiguous file runs with sorted/merged fallback for out-of-order layouts; defer borrowed-scanner evidence until a candidate can enter the page; retain exact evidence output and bounded source-buffer ownership. |
| K12 | `63fb0c0db304` | Broad relation-ID union scans once per segment, with sparse exact lookups when appropriate. Restore the native `BTreeMap` branch and borrowed bitmaps; encoded-ID scanning machinery is unnecessary. |
| K13 | `aaf732dd9329` | Correct segmented EXPLAIN candidates/exactness and FTS estimates; separate reverse posting-list counts from posting-member counts throughout static/segmented queries, semijoins, reports and tests. |
| K14 | `a35fb9ee3f0d`, `8747efc47ea4` | Keep metadata fast-path eligibility guards and direct timestamp comparison. Read native file revisions/record ranges; retain proper evaluator/error behavior for unsupported transforms, periods and operations. |
| K15 | `e2a09b1d3f18` | Restrict sparse relation scans to candidate files instead of scanning every segment file; use native record/file lookups and preserve revision checks. |
| K16 | `84bd746cff88` | Keep the top-page admission optimization: compare native sort keys/IDs first and postpone full result/catalog work until needed, including the verified-cache exception. |
| K17 | `6edb0349aac0` | Preserve bounded relation-worker scheduling and the 64-KiB record-range guard. Use native metadata or a cheaply maintained native maximum; remove any dependence on validated byte-backed tables. |

Important correctness detail for K10: the implemented optimization deliberately uses raw `presence`, not a blanket rewrite to `:exists`. The old task's prose is less precise than its final implementation about vacuous empty selections. Preserve the actual guarded implementation and its differential tests.

Retain the source-free metadata-FTS regression coverage introduced by `ea6ec585cfd0`. Preserve logical coverage, exactness and source-error behavior while simplifying checked accessors that existed only to decode external index bytes.

### 4.3 Mutation, partitioning and in-memory maintenance

| ID | Source commits | Preserve/reapply |
| --- | --- | --- |
| K18 | `14635a01a735`, `82cc05c21c11` | Reuse unaffected immutable shards and collection groups during compaction. Preserve pinned-view ownership/locality instead of rebuilding the whole publication. |
| K19 | `ed00d4987cd4` | One shared `PartitionRouting` source-slot map for partition metadata and projections. Preserve slot/domain validation and native metadata ownership. |
| K20 | `ff6c73e9478d` | Project gram postings across partitions in one pass. Adapt to native gram maps instead of serialized readers. |
| K21 | `e15b3f5c4b0e` | Scatter relation contributions once across partitions, preserving positions, unresolved identities and all presence/null/unavailable domains. |
| K22 | `980a8831dc98`, `5ca0598b0dcf` | Scatter sort orders, record domains, inferred/custom postings and inference facts once across partitions. Preserve local-slot mappings and coverage/completeness semantics. |
| K23 | `e8f2b1ea487e`, `8331da80d1e1` | Preserve count/retirement/size-driven maintenance and its hysteresis, triggers after pure deletion or lost collection membership, and retention of current zero-record file contributions. Replace encoded-byte accounting with a documented native retained-memory estimate. |
| K24 | `3bd8b74d795a` | Preserve bounded shard-batch compaction, cancellation, progress and remaining-retired-work reporting. Replace segment-file-byte charges with appropriate memory/source-work estimates; do not report these as strict allocator limits. |

The existing verified single-record/partition slot-preserving update paths already existed in `B`. Preserve them and the subsequent improvements above. Remove only their byte-backed-reader branches.

The current known limitations in `hybrid-index-storage-base-compaction-budget.md` and `verified-base-update-bounded-path-latency.md` must be recorded accurately. Restoring native trees does not establish that these separate issues are fixed. Preserve existing protections and avoid introducing new whole-collection work; assess the actual supported base/partition shapes in validation.

### 4.4 Frontend behavior, discovery and tests

| ID | Source changes | Preserve/reapply |
| --- | --- | --- |
| K25 | Progressive-open/frontend integrations and current desktop open/controller paths | Progressive catalog/query availability, cancellation, stale-worker rejection, source reconciliation, and keeping the current session visible until a replacement is ready. Preserve cleanup of a canceled newly opened session. |
| K26 | Current CLI definition preparation and source exclusions | Source/schema selection, `.git`/`.objekte` exclusions and applicable `objekte-indexes` artifact exclusions. Extract ordinary discovery rules from storage modules before deleting those modules. |
| K27 | Legacy removal and current frontend preferences | Keep automatic index-save preferences removed. Preserve saved queries, query history, layout, source-check policy, exports, completion, tabs and normal preferences. Old storage preference fields must not break unrelated preference loading. |
| K28 | Later semantic/oracle tests throughout core and clients | Preserve meaningful query, lifecycle and frontend assertions. Rewrite disk-roundtrip wrappers to construct fresh native indexes; delete serialization-only tests and helpers. |
| K29 | Current task reports, generated evidence and issue triage | Keep historical reports/results and completed removals, including issue moves. Update active instructions and report which persistence issues have become obsolete; do not erase the history. |

`03dc3e23bcf3` protected schemas against snapshot writes. Once every index writer is gone, its writer-specific protection API is obsolete. Preserve read-only source/schema behavior and ordinary export protections.

`5cda86d630f6` improved invalid-source-path error reporting inside storage preparation. Preserve useful source selection diagnostics in discovery where that preparation still applies, without retaining a storage resolver.

### 4.5 Changes whose purpose disappears

Remove these families rather than replaying them:

- Managed namespaces, generations, manifests, catalogs, current-generation pointers, locks, hard links, GC and validation stamps, including the disk portions of `8ca2851ab8d9`.
- Ordered encoded leaves, portable posting packs, byte-backed record/file tables, persisted membership/path/identity lookups, mmap readers and `Sealed*` adapters.
- Memory-only encoding/sealing and all encode/decode/materialize-back-to-builder transitions.
- Decoded-posting caches, descriptor proofs, serialized-key comparisons, encoded gram-presence filters and their optimizations (`33d1af0a8e35`, `78a33da69379`, `4a1c2a8883cc`, `01f83cf8f79b`, and related codec work).
- Disk-open corruption/migration/obsolete-format repair, flush retries, save workers and shutdown/destination state.
- Frontend storage parity and explicit persistence features, including `4d7d5b61bf27`, `05828120b803`, `31b0f67e46c4`, `89163825efab`, `6b78ba6cd632`, `8e2189e1eb9e`, `b54822f9afdf`, and the persistence parts of `1df5f2627491`.
- Persisted-open/save benchmarks and active storage-format documentation. Historical archives remain.

A commit being listed here does not override the carry-forward rules: inspect any surviving mixed-purpose changes before discarding it.

## 5. Restore the native core

### Representation and query consumers

Use the old implementations in:

- `crates/core/src/posting.rs` and `bigram.rs`.
- `index_catalog.rs`, `executor.rs`, `posting_values.rs`, `inference.rs`.
- `run.rs`, `relation_query.rs`, `relation_semijoin.rs`, `sorting.rs`.
- `mutation_segment.rs`, `lifecycle_partition.rs`, `lifecycle_compaction.rs`.
- `inventory.rs`, `lifecycle_catalog.rs`, lifecycle construction/publication.

Reapply K01–K29 as appropriate. Both static `Repository` and lifecycle `Index` must use the native representation, including incremental builds, custom source adapters, inferred/named bindings, mutations and compaction.

Remove the builder/sealed dual representation. Native maps are the completed query structures, not retained sidecars to an encoded index. Queries borrow existing bitmaps; mutations use existing segment/publication ownership. No new B+tree, storage dependency, generic backend interface or copy-on-write tree implementation is part of this task.

Preserve exact number ordering, typed scalar distinctions, text folding, normal/skip gram behavior, structural predicates, relation/array occurrence semantics, sort collation, active-record domains, bounded value-page cursors and stale-owner checks.

### Native metadata

Restore ordinary file/record tables and native lookup maps. Remove `persisted` alternatives and decode caches from `IndexedCollectionSegment`, `FileCatalog` and `FileVersions`. Keep useful in-memory catalog overlays/sharding and stable path/identity behavior from the native implementation.

Catalog changes and segment/posting changes must remain atomically published. An old pinned view must keep its old metadata and posting domains during moves, replacements, deletion and compaction.

The native timestamp, sparse relation and top-page optimizations in K14–K17 should use direct borrowing; recreating the byte-reader interface around native records would retain unnecessary machinery.

## 6. Remove persistence completely

### Modules

Delete the newer storage modules and their serialization-only tests:

- `managed_store.rs`
- `index_storage.rs`
- `lifecycle_storage.rs`
- `generation_catalog.rs`
- `hybrid_segment_codec.rs`
- `segment_format.rs`
- `segment_metadata.rs`
- `ordered_block.rs`
- `posting_dictionary.rs`
- `bigram_dictionary.rs`

Before deleting `index_storage.rs`, replace its readiness/status consumers with existing `IndexStatus`/`InitialBuildStatus` as specified in handoff D3. Do not retain or relocate `IndexReadiness` or the storage resolver. Preserve source exclusions through the source discovery rules in D2 and retain ordinary source/build/query errors.

The old snapshot modules must also be absent at completion:

- `storage.rs`
- `lifecycle_cache_guard.rs`
- `lifecycle_repository.rs` if it only serves cache-aware opening
- `lifecycle_segment_codec.rs`
- their obsolete tests and compatibility helpers

Keep repository configuration loading available through source-oriented APIs.

### APIs and state

Remove all generations of persistence interfaces, including:

- `Repository::save_snapshot*`, `load_snapshot*`, snapshot-policy opening methods.
- `SnapshotMode`, `SnapshotOpenPolicy`, `SnapshotAction`, snapshot configuration/profiling types.
- `Index::load_cache*`, `flush_cache*`, save-copy variants and `sync_cache` automation.
- Cache-aware `RepositoryOpenOptions`, cache open/failure policies and destination guards.
- `Index::try_load*`, `flush_to`, storage attach/status/origin methods and `IndexFlushReport`.
- Targets, residency/mode selectors, managed-generation IDs, durable revisions, saved destinations and unsaved-index flags.
- Manifest/segment/catalog encoders, checksums, format versions, compatibility probes, migrations and old-store cleanup.
- `enable_owned_memory_sealing`, `seal_current_segments_owned`, sealing switches and serialized payload accounting.

Remove persistence-specific shutdown-save flows. Keep `quiesce`, `resume_after_quiesce`, publication locking and normal worker shutdown because they protect live updates; handoff D3/D6 specifies their retained behavior.

Audit every public entry point and feature/test configuration. A hidden, deprecated, feature-gated or test-only old storage backend is still unfinished removal.

### Dependencies and identity

- Remove `memmap2` once its index-file reader is deleted.
- Audit Roaring/metadata serialization derives and features after removing index serialization.
- Keep `serde` where source formats, preferences, query protocols and diagnostics require it.
- **Do not blindly remove `bincode`: `verified.rs:163` currently uses it for an in-memory verified-query cache key.** That is not index persistence. Model validation tests also use it. Keep the dependency while those uses remain; changing their encoding is unnecessary.
- Keep hashing required by source fingerprints and live configuration identity. Delete disk payload checksums and persisted-format compatibility machinery.
- Preserve `MODEL_VERSION`, configuration-hash domain versions and source/text/collation identifiers used by live configuration identity (handoff D1). Delete disk format counters. Change CLI protocol 3 to 4 with its consumers (handoff D7).
- Remove any TOML persistence additions that are present on the actual execution base.

## 7. CLI, terminal UI and GUI

### CLI

Update `arguments.rs`, `loader.rs`, `execution.rs`, `output.rs`, `lib.rs`, process tests and documentation.

- Delete `--load-index`, `--load-index-directory`, `--flush-index`, `--flush-index-directory`, `--write-index`, `--index-storage`, `--index-directory`, `--index-residency`, and any historical storage aliases.
- Delete index-only save/load invocations and target/path parsing. Removed switches are ordinary unsupported arguments.
- Open source definitions into a fresh native index; retain folder/multi-folder/configured repository modes.
- Remove storage/load/save events and fields from human output and NDJSON, including `index_storage_action` if it only describes the deleted storage choice.
- Preserve source/build progress, inference, query diagnostics and current non-storage counters.
- Update protocol version from 3 to 4 and its consumers in the same checkpoint (handoff D7).
- Delete `storage_view.rs`; remaining clients use ordinary lifecycle error Display and their existing stage/kind mapping (handoff D3).

### CLI/TUI

Update `tui/app.rs`, `tui/mod.rs`, rendering, event handling, settings and tests.

- Delete storage overlays, destination selectors, open/save/retry commands and shortcuts.
- Delete storage workers/messages, pending requests, disk-status labels and save-on-exit flows.
- Preserve progressive construction, cancellation, search, completion, source checks, rebuild and in-memory maintenance.
- Keep source freshness distinct from the removed saved/unsaved-index state.
- Keep action menus coherent and preferences readable after the fields disappear.

### Native GUI

Update backend open/lifecycle, controller runtime/actions/messages, UI model/state, open dialogs, panels, render code, menus and preferences.

- Delete “Try loading saved index”, “Open saved index…”, “Save index…”, “Retry save”, storage targets/residency and storage-only dialogs.
- Delete the corresponding backend operations and callable actions, not just visible controls.
- Remove saved-origin/destination/generation/validation display and storage error/retry state.
- Source opening builds progressively in RAM. Preserve cancellation, current-session replacement behavior, query generations, tabs, source checks and maintenance.
- Close and repository switching stop workers and release memory without saving an index.
- Show useful build readiness, counts, source freshness and memory information in the Index panel.
- Keep normal preferences, saved queries, history and user-requested exports.

## 8. Artifact handling and no-index-I/O contract

Source discovery must skip applicable old generated-index locations before descending into them, including the established `.objekte` and `objekte-indexes` namespaces. These are ordinary exclusions, not a storage discovery/probing subsystem.

The application must not open, create, update, validate, migrate or clean old snapshot files, manifests, catalogs, generation directories, `.idx` files, temporary/custom index stores or serialized index buffers.

Exercise startup with absent, malformed and inaccessible old stores and confirm identical source-build behavior. Account for unavoidable parent-directory enumeration separately from deliberately probing an old store.

Inventory existing generated artifacts and report their exact paths for one-time cleanup. This task's functional removal must not grow a shipped migration/cleanup command. Preserve source files, `.objekte/collections`, normal preferences and unrelated user files.

Low-memory handling must remain memory-only; no scratch index files or spill fallback.

## 9. Implementation sequence

**Execution order is C0–C5 in the handoff.** The phases below describe logical work groups, not mandatory separate commits. C1 first decouples frontend/test callers using existing memory entry points, then C2 performs the tightly coupled native-core conversion and complete module removal. Each completed checkpoint has a defined compilation/test gate.

### Phase A — freeze references and preservation ledger

1. Record actual execution base `H'`, branch/worktree and clean/unrelated working state.
2. Keep `B` and `H'` available as reference revisions.
3. Extend the audit over `H..H'`, if nonempty.
4. Use the complete companion ledger for `B..H`; extend it for `H..H'`, if nonempty. Verify the handoff's changed-path ownership table against the actual base.
5. Use the handoff's K01–K29/T1–T9 map and append final implementation evidence. The audited merge-only resolutions are recorded in the companion ledger; audit newer merges too.
6. Capture current memory-build/query/update measurements before deleting the old benchmark options. Use isolated source fixtures for mutation measurements.

Useful read-only inventory:

```sh
BASE=da8fde68472b8646a63a05b3b3654c58de877301
START=$(git rev-parse HEAD)
git log --reverse --topo-order --format='%H %P %s' "$BASE..$START"
git diff --name-status "$BASE" "$START"
git diff "$BASE" "$START" -- crates
```

Run this inventory before implementation; record `START` as `H'` for subsequent comparisons.

### Phase B — restore native index construction and querying

1. Restore native posting and bigram maps, native metadata and direct borrowing/range access.
2. Adapt construction, incremental publication, queries, relations, sorting and value enumeration together.
3. Carry forward structural/self-comparison/scanner/diagnostic improvements.
4. Carry forward one-pass partition projection, shard reuse and live maintenance behavior with native accounting.
5. Remove serialized memory sealing and byte-backed branches.
6. Ensure ordinary memory entry points are functional before proceeding.

### Phase C — delete persistence end to end

1. Remove every core storage API/module/state field and storage-only dependency.
2. Preserve configuration identity, discovery rules and build readiness in their appropriate modules.
3. Remove CLI/TUI/GUI storage behavior and update protocol/preferences.
4. Remove obsolete examples/helpers; migrate semantic tests to memory-only construction.
5. Update active documentation and record artifact locations.

Phases B and C may share compilation-coherent commits where interfaces are tightly coupled. No completed checkpoint may claim memory-only delivery while a storage backend remains.

### Phase D — verify carry-forward completeness

1. Check K01–K29 against the implementation and meaningful tests.
2. Review the full diff against `H'`: every lost non-storage behavior must be restored.
3. Review the full diff against `B`: every addition must be an identified retained improvement or necessary persistence-removal adaptation.
4. Confirm binding-cache deletion, old snapshot deletion and issue/report history survived.
5. Confirm no accidentally restored UI regressions, obsolete flags, baseline cache defaults or codec-dependent test helpers.

### Phase E — combined acceptance and completion

Run the checks below, resolve regressions introduced by this task, record retained known limitations, and complete the repository report/commit/push workflow on the integration branch.

Suggested commit groups: native representation restoration; query carry-forwards; native maintenance carry-forwards; complete core persistence deletion/cache-removal preservation; frontend removal; test/benchmark/docs cleanup; combined validation/report. Adjust boundaries to keep commits coherent.

## 10. Validation

### Build and semantic checks

Run the workspace suite and applicable checks after the combined changes:

```sh
cargo test --workspace --locked -- --quiet
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
git diff --check
```

Use repository formatting checks and report any pre-existing global drift without bundling an unrelated reformat.

Retain/adapt existing tests; add focused regressions only for meaningful coverage gaps:

- Static and incremental native construction, including custom readers.
- Scalar/length ranges, exact numeric ordering, reverse/prefix value pages and cursor ownership.
- Structural predicates over declared unindexed fields and explicit array steps.
- Guarded self-comparison, including missing/null/empty-selection/array/object fallback cases.
- Scanner evidence and out-of-order file grouping.
- Metadata filters/FTS, exact count, sorted pages, relations and candidate diagnostics.
- Fresh query binding, regex/parameter handling and removed cache accounting.
- Add/change/delete/rename, zero-record files, verified update fallback, shard reuse, partition mappings, cancellation and pinned views.
- In-memory compaction without source rereads; partial-work reporting and native memory accounting.

Remove `crates/core/tests/support/legacy_storage.rs` and core storage test adapters. Do not keep a serializer merely to construct fixtures or compare entire indexes; compare logical results/domains instead.

### Query and performance comparisons

Use the existing Musenalm benchmark/oracle infrastructure:

- Complete extended **220-case ASCII and 220-case Unicode** comparisons against `H'`, including counts, ordered pages and evidence.
- Existing independent `jq` oracle and reference-evaluator coverage.
- Fresh static/incremental construction, time to catalog/search/full readiness, inference off/on.
- Representative narrow/wide ranges, common/absent grams, metadata, sorts, relations and posting-value pages.
- Mutation/locality/verified-update/compaction campaigns on disposable copies, including pinned old views.
- Peak build memory, retained memory, allocation work and release after dropping old views.

Use `bench_musenalm`, `bench_build_musenalm`, `bench_index_construction`, `bench_initial_concurrency`, `bench_lifecycle_mutations` and the existing regression scripts as applicable. Inspect and adapt their current arguments; remove storage options from runnable scripts.

`H'` is the acceptance baseline for retained behavior and current memory-mode performance. `B` is an architectural/native performance reference. Its old default snapshot policies must be disabled explicitly when measuring it, and its old binding-cache behavior must be accounted for.

Compare fresh memory construction with fresh memory construction, never with saved-index opening. Use ordinary release binaries for timings and feature-gated instrumentation separately. Keep fixtures, workers, inference and verified-cache settings matched; disable verified-result reuse for the main query correctness/performance comparison.

No new hard percentage budget is imposed by this plan. Report paired per-scenario timing/memory results, investigate material regressions, and do not hide a query/update regression behind faster startup. The earlier speculative B+tree and disk-load percentage budgets do not govern this restoration.

### Actual frontends and filesystem checks

- CLI: supported inputs, removed flags, help, diagnostics and NDJSON.
- TUI: open, incremental availability, query/completion, source check, rebuild, action menus, cancel and close through existing PTY facilities.
- Native GUI: source open, progressive results, query/completion, count/page, source update/rebuild, repository switching, cancellation and close.
- Use `docs/development/gui_environment.md` and the maintained native desktop launcher/control scripts; backend unit tests alone do not establish UI removal.
- Check absent, malformed and inaccessible old index artifacts.
- Combine before/after filesystem inventories with focused file-access tracing. Confirm no saved-index probes/reads/writes on ordinary open, query, update, maintenance or shutdown.
- Allow legitimate source reads, normal settings, user-requested exports and diagnostic output.

## 11. Completion criteria

1. Native B-tree scalar/length/structural postings and native hash-map grams are the completed query representation.
2. Static, incremental and mutation paths publish native indexes directly.
3. No serialized-in-RAM snapshot representation, byte-backed index reader or decoded-posting cache remains.
4. Both old snapshots and newer managed index persistence are absent from core APIs, implementation, tests/helpers and runnable examples.
5. CLI, CLI/TUI and GUI expose no storage flags, actions, dialogs, worker operations or automatic save/load behavior.
6. Prepared-query binding remains uncached; all unrelated K01–K29 behavior is retained or reintroduced with traceable evidence.
7. Source configuration, schema protection through read-only behavior, source freshness, progress, cancellation and pinned-view semantics are preserved.
8. Workspace checks, semantic campaigns, native frontend checks and no-index-I/O verification pass.
9. Comparable construction/query/update/memory measurements and existing limitations are reported.
10. Final review accounts for every post-`B` change in the execution range, including merge resolutions and later work.
11. Current docs describe memory-only operation, historical records remain available, and the integration branch/report are committed and pushed under the repository workflow.

## 12. Principal references

- `docs/ai/done/objekte_hybrid_index_storage_implementation_plan.md`
- `docs/ai/done/objekte_hybrid_index_storage_repair_specification.md`
- `docs/ai/done/objekte_hybrid_index_allocation_and_query_performance_specification.md`
- `docs/ai/done/hybrid-store-generation-validation-readpath-plan.md`
- `docs/ai/done/query-scanner-plans-plan.md`
- `docs/ai/done/frontend-storage-parity-plan.md`
- `docs/ai/done/core-compile-warnings.md`
- `docs/ai/done/objekte_direct_snapshot_access_specification.md`
- `docs/ai/done/remove-binding-cache-plan.md`
- `docs/ai/done/remove-legacy-snapshot-stack-plan.md`
- `docs/ai/done/objekte_explicit_index_persistence_implementation_plan.md`
- `docs/development/gui_environment.md`
- `bench/README.md`

Planning modified only files in `/home/simon/.opencode/plan`. No repository code, branch, task record, index artifact or Git history was changed.

### Hardening pass

The companion documents add the complete 236-row history disposition, two explicit merge-resolution decisions, production-file instructions, resolved API/ownership decisions, nine regression groups, exact semantic-test migrations, five implementation checkpoints after baseline capture, and final positive/negative audits. They reduce implementation judgment; they do not substitute for running the acceptance checks.

Planning validation: the ledger matches the exact topological B..H commit list with no missing/duplicate rows and correct merge classifications; all referenced commits and companion links resolve; all 44 exact existing regression names in the handoff matrix exist; all 74 changed production Rust paths are named by the file instructions/removal lists. Application tests/benchmarks are execution gates, not checks claimed to have run during this documentation-only pass.

## Implementation report

- Task identifier: `restore-native-btrees-memory-only`
- Completion date: 2026-11-14
- Starting revision (`START`): `66b932f31f922a13062a61658e843e3033b3aff4`
- Native reference (`B`): `da8fde68472b8646a63a05b3b3654c58de877301`
- Audited preservation reference (`H`): `a0a0691897c6ff24985fc4db9ae51f5d77a89899`
- Integration branch: `integration/restore-native-btrees-memory-only`
- Integration worktree: `/home/simon/source/objekte-integration-restore-native-btrees-memory-only`
- Implementation commits: `0ddb7f783dbb97adcbb85f23f5133d5ffd5a1921` (native restoration and persistence removal), `14a9e534ebc3cf11dd881b24f46c996be182efff` (benchmarks and active docs); this report is a later commit on the same branch.

The two companion documents were supplied after the initial planning read and were used as the controlling specification. The handoff's C0–C5 order and D1–D7 decisions were followed; C1 was folded into the same working change because the caller and core interfaces are coupled.

### Summary

The finished application builds its index from source files into RAM on every opening. Scalar/length postings are `BTreeMap`s, grams are `HashMap`s, segment metadata is natively owned, and queries borrow those structures directly. All disk-index persistence is gone: no saved-index load/save, import/export, automatic synchronization, shutdown save, recovery, disk cache, mmap reader, encoded in-memory representation, decoded-posting cache or index-only CLI mode remains.

Removed modules and their tests: `managed_store`, `index_storage`, `lifecycle_storage`, `generation_catalog`, `hybrid_segment_codec`, `segment_format`, `segment_metadata`, `ordered_block`, `posting_dictionary`, `bigram_dictionary`. The legacy stack (`storage`, `lifecycle_cache_guard`, `lifecycle_repository`, `lifecycle_segment_codec`) remains absent. `test_support.rs` and `tests/support/legacy_storage.rs` were deleted as storage-only bridges.

Removed APIs and state include `IndexStorageTarget`, `ResolvedIndexStorage`, `IndexFlushReport`, `IndexStorageStatus`, `IndexStorageFailureKind`, `IndexSynchronization`, `IndexReadiness`, `Index::try_load*`, `flush_to`, `flush_storage`, `attach_storage*`, `storage_status`, `restored_from_storage`, `loaded_origin`, `saved_revision`, `IndexStatus::dirty`, `SnapshotMode`/`SnapshotOpenPolicy`/`SnapshotAction`, `enable_owned_memory_sealing`, `seal_current_segments_owned`, `DecodedPostingCacheUsage`, `IndexError::{InvalidCache, StoreConflict}`, the managed-generation owner/catalog types and `memmap2` as a direct dependency.

### K carry-forward evidence

| K | Evidence |
| --- | --- |
| K01 | Fresh preparation and binding logic retained in `prepared.rs`; no `PreparedCache`, budget, hit-counter or retained-size symbols remain; `parameters`, `regex_record_id`, `temporal_regex_integration` and `streaming` tests pass. |
| K02 | Feature-gated preparation timing and fresh-binding verification remain; historical binding-cache measurements are unchanged under `bench/results` and `docs/ai/generated`. |
| K03 | `configuration.rs` still owns live identity; `live_configuration_hash_matches_the_pre_restore_fixture` proves byte-identical hashes for unchanged inputs. |
| K04 | Legacy snapshot stack and its TUI/desktop actions remain absent; `manual_close_does_not_write_index_artifacts` and the process tests cover ordinary close. |
| K05 | `AppendShiftedBitmap` uses `batch.logical_records()`; `inference_copy()` is a separate timed observation in `repository_incremental.rs`. |
| K06 | `InitialPublicationPhases` keeps preparation/install timing and drops `sealing_ns`; `IndexTimingsMs`, build provenance and benchmark binary hashes are retained. |
| K07 | Dead codec helpers removed; clippy reports 284 warnings versus 320 at START, with no warning suppression added. |
| K08 | `PostingIndex` keeps `values_indexed`, structural facts for every declared field and auto-index exclusion rules; T1 tests pass. |
| K09 | Declared array-step state routing preserved in `index_catalog.rs`; segmented structural tests pass. |
| K10 | Guarded raw-presence self-comparison and EXPLAIN reasons preserved in `index_catalog.rs`; differential tests pass. |
| K11 | Inline single-file ranges, grouped file runs, lazy evidence and bounded source buffers preserved in `run.rs`; scanner tests pass. |
| K12 | `PostingIndex::union_id_postings` is native, returns `(union, lists, members)` and keeps the sparse/broad threshold; `broad_id_union_matches_individual_lookups` passes. |
| K13 | Segmented EXPLAIN candidate/exactness and separate relation list/member counters preserved; relation and diagnostic tests pass. |
| K14 | Metadata fast-path guards and direct timestamp comparisons preserved; native `file`/`record` accessors replace byte readers. |
| K15 | Sparse relation scans remain candidate-file scoped with native revision checks. |
| K16 | Top-page admission postpones record/catalog work until a candidate enters the page; stored-order and page-evidence tests pass. |
| K17 | 64-KiB relation scheduling guard reads cached `SegmentAccounting::max_record_range_len`; no per-query record scan. |
| K18 | Unaffected shard/collection-group `Arc` reuse preserved; `compaction_reuses_unaffected_native_segment` asserts `Arc::ptr_eq` and unchanged accounting. |
| K19 | Shared `PartitionRouting` slot map and validation preserved in `lifecycle_partition.rs`. |
| K20 | One-pass gram projection works on native gram maps (`project_bigrams_for_partitions`). |
| K21 | Relation contributions scatter once with positions and presence/null/unavailable domains. |
| K22 | Sort orders, record domains and inferred/custom postings scatter once; inference projections remain. |
| K23 | Count/retirement/size triggers, hysteresis and current zero-record files preserved; size accounting is the cached native estimate. |
| K24 | Bounded compaction batches, cancellation, CAS retry and remaining-retired reporting preserved; budget fields are native estimates. |
| K25 | CLI/TUI/desktop progressive source open, stale-worker rejection, cancellation and old-session retention preserved; desktop runtime tests pass. |
| K26 | `.git`, `.objekte` and `objekte-indexes` exclusions remain; `prepare_source_definitions` validates selections and adds generated-index exclusions; discovery prunes generated namespaces before descent; process fixtures prove poisoned artifacts are skipped. |
| K27 | Saved queries, history, tabs, layout, source-check policy, exports and normal preferences remain; storage/autosave fields are absent and old unknown fields stay ignored. Desktop preference tests pass. |
| K28 | Core, integration, CLI and desktop semantic suites pass; disk-roundtrip wrappers were replaced with fresh native builds or same-index assertions. |
| K29 | `docs/ai/done`, `docs/ai/generated` and `bench/results` history is untouched except for this task's completion documents; active docs were updated; unrelated issue documents remain. |

All ledger `D` rows are closed by the module/API audit below, `H` rows by the unchanged history tree, `M/MR` rows by starting at START and not replaying historical merges, and `A/K` rows by the surviving code and tests above. No commit was reverted or cherry-picked wholesale.

### Native ownership and interface audit

- `PostingIndex.values: BTreeMap<ScalarKey, RoaringBitmap>`, `lengths: BTreeMap<Number, RoaringBitmap>`; presence/null/empty-selection/structural bitmaps are directly owned. No `sealed` field or checked dictionary wrapper.
- `BigramIndex.normal`/`.skip: HashMap<u64, RoaringBitmap>`; no sealed reader, presence filter or decoded cache.
- `IndexedCollectionSegment.files`, `file_index`, `records`, `id_lookup`, `identity_order` are native owned data. `record`, `file` and `file_records` return borrowed references; `try_*`/`Cow` adapters are gone. `SegmentAdmission::finish` is the native finalizer.
- `FileCatalog` keeps its native base and bounded changed/new/path overlays; `PersistedBase`, row caches and `restored_persisted` are gone.
- `FileVersions` uses native shared chunks only; persisted base/override branches are gone.
- `cheapest_consolidation_pair` returns `Option<(usize, usize)>` and inspects native file/ID maps.
- `SegmentAccounting { retained_bytes_estimated, source_bytes_estimated, max_record_range_len }` is cached on each finalized segment. New, compacted, consolidated and remapped segments recompute it; `Arc` reuse preserves it; active-mask-only retirement keeps the physical size. `input_bytes_budgeted = max(retained, source)` and `temporary_bytes_estimated = input * 3 + physical_records * 512`.
- `.objekte/.indexes`/`.objekte/indexes` are added to discovery exclusions by `repository_configuration::prepare_source_definitions`, idempotently and in the same order as START. Live configuration identity is byte-identical (fixtures `d025ae2e…` and `0583c24f…` matched START and candidate).

### CLI, TUI and GUI

- CLI has no `--load-index`, `--load-index-directory`, `--flush-index`, `--flush-index-directory`, `--write-index`, `--index-storage`, `--index-directory` or `--index-residency`; process tests assert exit 2 and no artifacts. It always opens a progressive source build, keeps query previews, ordering, cancellation and exit status, and emits protocol version 4.
- TUI has no storage overlay, target selector, save/open/retry commands, storage workers or save-on-exit flow. PTY validation showed progressive construction, two FTS matches, and an action menu containing Collections, Sort, Columns, History, Saved queries, Save query, exports, case/regex, preview, theme, help and files with no index-storage action.
- Desktop `OpenSpec`/`OpenedSession` no longer carry a load target; opening always starts the source builder, and replacement/cancellation/stale-worker handling remains. The application menu has Check for changes, Index…, exports, batch, diagnostics and help but no saved-index open/save/retry. The Index panel showed Searchable readiness, source freshness, revision, `1.4 KiB` estimated memory, file/record counts, Rebuild/Compact and file-check results.

### Test migration

Required renames:

| Old | New |
| --- | --- |
| `compacting_sealed_segments_preserves_text_postings` | `compacting_native_segments_preserves_text_postings` |
| `compaction_reuses_unaffected_sealed_segment` | `compaction_reuses_unaffected_native_segment` |
| `compacted_base_contributions_round_trip_through_managed_store` | `compacted_base_contributions_preserve_native_queries` |
| `partition_and_managed_restore_keep_global_inference_diagnostics` | `partition_and_native_publication_keep_global_inference_diagnostics` |
| `partial_structural_postings_survive_explicit_mutations_and_cache_roundtrip` | `partial_structural_postings_survive_explicit_mutations_and_compaction` |
| `inferred_order_cache_reuses_pages_and_restores_without_persisting_orders` | `inferred_order_cache_reuses_pages_and_is_scoped_to_native_instance` |
| `cache_exclusion_precedes_seen_tracking_and_scan_cancels_between_batches` | `generated_directory_exclusion_precedes_seen_tracking_and_scan_cancels_between_batches` |
| `failed_saved_index_open_keeps_previous_memory_session` | `failed_source_open_keeps_previous_session` |
| `configured_memory_open_ignores_existing_deprecated_snapshot` | `source_open_ignores_existing_index_artifacts` |
| `opens_folder_without_implicit_snapshot_write` | `opens_folder_without_index_artifacts` |
| `manual_close_has_no_implicit_index_save` | `manual_close_does_not_write_index_artifacts` |
| `schema_bytes_and_inference_mode_select_distinct_managed_indexes` | `schema_bytes_and_inference_modes_rebuild_from_source` |
| `manual_and_automatic_postings_reopen_together` | `manual_and_automatic_postings_build_together` |
| `removed_residency_flag_is_rejected_with_migration_guidance` | `removed_storage_flags_are_unsupported_arguments` |
| `bare_obj_errors_but_explicit_flush_allows_index_only` | `bare_obj_requires_a_query` |
| `empty_query_file_is_an_input_error_unless_index_only_was_requested` | `empty_query_file_is_an_input_error` |
| `diagnostic_disk_runs_report_timing_across_a_reopen` | `diagnostic_source_runs_report_timing` |
| `diagnostic_lists_inferred_btree_paths_after_build_and_reopen` | `diagnostic_lists_inferred_btree_paths_after_a_source_build` |
| `custom_index_namespace_does_not_hide_unrelated_source_files` | `generated_index_namespaces_are_excluded_from_source_discovery` |
| `mixed_short_strings_match_reference_after_managed_restore` | `mixed_short_strings_match_reference_across_static_and_native_publication` |
| `direct_json_build_matches_static_inference_overlap_and_managed_restore` | `direct_json_build_matches_static_inference_overlap_and_native_publication` |

Deleted storage-only tests include the managed load/save/generation, custom-target picker, save-worker/retry-close, snapshot-artifact round-trip, codec corruption/layout/format/locking/stamp/flush-retry suites, `sealed_broad_id_union_uses_checked_borrowed_keys`, and storage-backed tests in `index_storage`, `managed_store`, `lifecycle_storage`, `generation_catalog`, `hybrid_segment_codec`, `segment_format`, `segment_metadata`, `ordered_block`, `posting_dictionary` and `bigram_dictionary`. Semantic assertions were retained in native tests.

### Validation

Commands run in the integration worktree:

```sh
cargo test --workspace --locked --no-fail-fast
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
git diff --check
```

Results: workspace tests pass — core 508 passed/15 ignored; CLI 81 passed, process suite 33 passed/1 ignored plus 1 removed-flags test; desktop 159 passed/1 ignored; all integration suites and doc tests passed. Check, detailed-profiling check and `git diff --check` pass. Clippy has 284 warnings and no errors; START had 320 warnings, so no new warning was added, and the repository's formatter reports the same pre-existing global drift (79 files at START) rather than an unrelated reformat.

### Query and performance evidence

- Extended Musenalm campaign: 220 cases × ASCII/Unicode × 5 repetitions, 440 `(case_id, profile)` rows and 220 result digests. All 440 statuses/counts/pages and all 220 digests match START; median candidate/START timing ratio 0.94. One campaign outlier (Q037/Q041/Q042/Q062/Q170/Q172) was re-run nine times; Q172 measured 387.8 µs at START versus 377.1 µs candidate, so no material regression was reproduced. The isolates were recorded under `/tmp/opencode/restore-btrees/results/q172-*`.
- Independent jq oracle: 150 predicates × ASCII/Unicode, zero membership mismatches.
- Fresh build gate (3 repetitions): ASCII median 0.962→0.984 s, peak RSS 460.7→467.5 MiB; Unicode 0.985→1.003 s, RSS 484.1→479.1 MiB. Both binaries reported 148,486 parses and zero failures.
- Lifecycle mutation small fixture: candidate build 3.62 ms (START 5.75), add 0.245 (0.295), append update 0.130 (0.111), verified update 0.124 (0.106), move 0.055 (0.036), verified delete 0.035 (0.021), compact 0.001 (0.001); candidate also completed the source-free no-change reconciliation in 0.110 ms. START failed at its legacy managed flush, which is why only the native phases form the comparison.
- Initial concurrency `build_ns`: idle 1.199→1.095 s, foreground 1.143→1.006 s, held-view 1.118→1.001 s.
- Correctness baselines: the extended campaign digests, jq membership and the workspace query suites cover counts, ordered pages, evidence, relations, ranges and metadata paths.

### Frontend and no-index-I/O evidence

- CLI process tests cover supported inputs, removed flags, help/diagnostics and NDJSON protocol 4.
- PTY TUI: progressive build, `needle` search returning two matches and the complete action menu without storage actions.
- Native desktop in headless Sway: source open, catalogue, FTS query with two matches, application menu without storage actions, Index panel (Searchable, source freshness, revision 3, 1.4 KiB estimate, counts, Rebuild/Compact, file check) and a successful “Check for changes” run.
- `strace` of open/query on a fixture seeded with `.objekte/.indexes/manifest.toml`, `.objekte/indexes/*.idx`, `search-core-ascii.snapshot` and `cache/objekte-indexes/*.idx` recorded no open/stat access below any generated namespace; the fixture inventory (paths, sizes, modification times) was unchanged. Absent, malformed and permission-inaccessible old stores all produced the same two-hit source build and exit 0.

### Historical records, obsolete issues and limitations

- Historical task reports, generated evidence, benchmark archives and issue triage are unchanged. The following issue reports describe persistence behaviour that this task removes and are now obsolete as active work: `issues/perf/hybrid-index-storage-fresh-disk-sealing-path.md`, `issues/feat/hybrid-store-failure-validation-coverage.md`, `issues/feat/explicit_save_retry_prefills_previous_path.md` and `issues/bug/fragmented-loaded-save-identity-mismatch.md`. The compaction/update-latency and metadata-performance issues are retained; native restoration does not establish that they are fixed.
- No automatic migration or artifact-cleanup command is shipped. Observed generated artifacts for optional one-time manual cleanup: `/home/simon/source/objekte/examples/musenalm/.objekte/search-core-ascii.snapshot`; `/home/simon/source/objekte-integration-hybrid-index-storage/examples/musenalm/.objekte/search-core-ascii.snapshot` and `search-core-unicode.snapshot`; and the 250 MB system temporary managed store `/tmp/objekte-1000`. `.objekte/collections`, `.cache`, `views`, source files and preferences were preserved.
- Known limitations retained from the completed task records: base-containing compaction cost and verified-update bounded-path latency are unchanged. No native memory accounting is a strict allocator/RSS bound, and the sparse-size trigger uses the documented planning estimate.
- Native GUI validation initially required the task-owned headless Sway environment; after starting it, the production desktop binary was exercised as described. The integration worktree's `target` is a temporary symlink to the original checkout's ignored validation tree and is not committed.

### Cleanup

The integration worktree and branch are retained for the user's merge. No subordinate worktrees were created. Temporary task memory under `docs/ai/memory/restore-native-btrees-memory-only/` is deleted after this report is committed and pushed.

## Review-completion addendum — 2026-11-14

Reviewed head: `6184a89e1954f80bd7a489288a9b9bb6bd08e241`.
Completion implementation commits: `2a2c12189d5a4a48ff644d6cf744c6a837b7d3d5`
(review fixes), `4078a3c5d65446b413ac343a2701316e3c66097a` (H..START ledger
appendix), followed by the acceptance-evidence and report commits on the same
integration branch. The branch remains unmerged.

This addendum supersedes earlier completion claims where they were broader than
the evidence supported:

- The earlier “no new warning” statement rested on the lower total count. The
  correct comparison is by diagnostic/file/message, accounting for deleted
  code: START had 299 actual Clippy diagnostics, final code has 221, and after
  normalizing the three variant-size messages there are **zero new diagnostic
  categories**. The follow-up also removed needless-borrow and unnecessary-cast
  warnings introduced by the native accessor conversion.
- The earlier GUI matrix did not exercise a nontrivial corpus with idle polling,
  check, compaction and cancellation. Those scenarios are now recorded in the
  durable evidence package.
- The earlier no-index-I/O evidence covered open and query. It now also covers
  mutation, compaction, rebuild, reconciliation, shutdown and symlink aliases.
- Query/build numbers were rerun on the final code: 440/440 results and 220/220
  digests still match START; final median ratio 0.922 and geometric mean 0.857.

### Finding 1: GUI status polling traversed the index

`update_index_ui` evaluated `IndexView::estimated_index_bytes()` before its
unchanged-status guard, and that estimate traversed gram/posting dictionaries
and sort keys every ~100 ms.

Fix: `CollectionState` now caches its narrow index-family bytes at build
finalization, and `SegmentAccounting` carries `index_family_bytes` recomputed
whenever a segment is finalized, consolidated, compacted or remapped. `Arc`
reuse retains the cached value. `IndexView::estimated_index_bytes()` is now an
O(collections + segments) sum of those cached values, and the GUI estimate
remains explicitly a planning estimate, not RSS.

Verification: `status_estimate_reuses_cached_native_accounting` resets a
thread-local estimator counter after build, calls the estimate twice and
asserts zero estimator calls; it then adds a file, proves the estimate grows and
asserts polling over the new publication again performs zero dictionary
traversal. The production GUI was exercised in headless Sway with a configured
200,000-record / 100-file JSONL repository. The Index panel showed
`Estimated index memory: 5.5 MiB`, Searchable readiness, revision 4 and
100 files/200,000 records. Idle CPU with the panel closed/open was 1.4%/1.4%
for the reviewed binary and 1.6%/1.6% for the candidate debug binary; GUI
repaint dominates that idle measurement, so the no-traversal claim rests on the
instrumented regression test, not a CPU delta.

### Finding 2: generated namespaces were inconsistent

The walkers pruned `.objekte` and `objekte-indexes`, but direct selection,
prepared scan selection, mutation preparation, source-root validation and
followed aliases could still admit generated content.

Fix: one generated-directory rule now applies to `selects_file_lexical`,
`selects_file`, `PreparedScanSelection`, both discovery walkers,
`discover_stream`, `discover_bounded`, `prepare_source_definitions` and
`FollowWithinRoot` physical aliases. Reserved source roots and descendants fail
with a contextual source-selection error; aliases into a generated namespace are
skipped or rejected. Legitimate siblings, `.objekte/collections` schema loading,
recursion/hidden/glob/symlink semantics and live configuration hashes are
unchanged.

Verification: `generated_namespaces_are_rejected_by_direct_selection_and_aliases`,
`followed_aliases_into_generated_namespaces_are_not_sources`,
`prepared_scan_selection_matches_generated_namespace_eligibility`,
`source_selection_rejects_generated_namespaces_and_aliases`,
`generated_namespaces_are_rejected_by_mutation_and_reconciliation` and the CLI
process fixture. The mutation test covers explicit add, move, update and
reconciliation; the discovery tests cover recursive/nonrecursive, hidden and
alias cases.

### Correction after external review

`selects_file` now rejects a canonical source root that resolves into a
generated namespace, so an alias source root cannot expose
`alias/poison.json` under either symlink policy, including when an already-open
source root is retargeted. `canonical_alias_source_roots_are_rejected_for_direct_selection`
covers `SymlinkPolicy::Skip` and `FollowWithinRoot`.

Both followed walkers now test the reserved directory name before calling
`canonicalize`, so a dangling or looping `objekte-indexes` link is pruned
instead of aborting discovery. `followed_discovery_ignores_reserved_dangling_and_looping_links`
and `stream_discovery_ignores_reserved_dangling_and_looping_links` cover the
bounded and streaming paths.

### Finding 3: focused regression gaps

- **Metadata parity:** `metadata_fast_paths_and_fallbacks_match_the_reference_evaluator`
  compares supported timestamp/size comparisons and unsupported state,
  `:lower` transform and regex matcher cases against `search_reference`;
  supported metadata-only queries retain zero source file opens and record
  parses.
- **Scheduling boundary:** `record_range_scheduling_boundary_preserves_results_and_cancellation`
  builds exact 65,536- and 65,537-byte records, asserts the cached maximum, runs
  one- and four-worker queries with equal results, and verifies a pre-cancelled
  stream reports `StreamOutcome::Cancelled`. The scheduler helper
  `record_ranges_allow_parallel_scan` uses an inclusive 64-KiB boundary.
- **Native accounting:** `native_accounting_grows_with_postings_and_survives_retirement`
  asserts nonzero retained/source/family estimates, growth after additional
  postings, unchanged accounting after active-mask retirement, and non-zero
  refreshed accounting after compaction. The sparse-trigger hysteresis test
  operates on a staged root without source reads.
- **Frontend compatibility:** removed flags, protocol 4 and storage-action
  absence remain covered; `legacy_storage_fields_do_not_discard_ordinary_settings`
  proves old storage/autosave fields do not discard ordinary desktop settings.

### Verification and evidence

Final checks on the review code: workspace tests passed (core 515/15 ignored,
CLI 81 + 33 process/1 ignored + 1 removed-flags, desktop 160/1 ignored, all
integration and doc tests); workspace all-targets check, detailed-profiling
check and `git diff --check` passed. The repository's pre-existing global
formatter drift was recorded rather than reformatted.

Final matched query/build results:

- 440/440 status/count/page rows and 220/220 result digests equal to START.
- Candidate/START median query timing 0.922; geometric mean 0.857; 256 cases
  >5% faster, 138 within ±5%, 46 >5% slower (the prior Q172 outlier re-ran at
  0.72× ASCII / 1.01× Unicode).
- Incremental open 1.401/1.593 s → 1.094/1.034 s; process RSS
  568.3/1049.4 MiB → 541.7/1033.2 MiB.
- Fresh-build gate: ASCII 0.962→0.919 s, Unicode 0.985→0.969 s; peak RSS
  460.7→465.7 and 484.1→480.1 MiB.
- jq oracle: 150 predicates × two alphabets, zero mismatches.
- Initial-concurrency build: 1.199→0.969 s idle, 1.143→0.980 s foreground,
  1.118→1.025 s held view.

The durable evidence package is
`bench/results/restore-native-btrees-memory-only/`: exact commands and working
directories, benchmark binary hashes, corpus identity, result tables, digests,
run manifests, raw repetition samples, lifecycle/concurrency outputs, GUI
screenshots, TUI captures and no-index-I/O traces. Large binaries and the source
corpus are not committed. `query-plans.jsonl` is omitted; the remaining outputs
are retained.

The H..START disposition is appended to the commit ledger in
`docs/ai/done/restore-native-btrees-commit-ledger.md`; the merge was clean and
the TOML persistence work is removed while its reports, measurements and source
exclusion ordering remain accounted for. The native GUI debug build for the
148k corpus did not complete within the practical acceptance window, so the
polling/frontend matrix used a 200k-record configured fixture; this is recorded
as an evidence limitation, not a passing claim. The compacted-base and
verified-update bounded-path limitations remain open.
