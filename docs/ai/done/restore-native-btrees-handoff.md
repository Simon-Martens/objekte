# Native B-tree restoration: implementation handoff

Date: 2026-11-14. Status: specification only; no implementation authorized by this document's existence.

Read together, in this order:

1. Repository `AGENTS.md` and `docs/ai/README.md`.
2. [Main plan](restore-native-btrees-memory-only.md): outcome, baseline and K01–K29 requirements.
3. This handoff: resolved decisions, file instructions, checkpoint order and regression gates.
4. [Complete commit ledger](restore-native-btrees-commit-ledger.md): every one of the 236 B..H commits.

This handoff **replaces open-ended choices** in the earlier plan. Its decisions and checkpoint order take precedence over earlier “assess/adapt/preserve” wording. User instructions and repository workflow take precedence over all three documents.

## 0. Execution rules

- `B = da8fde68472b8646a63a05b3b3654c58de877301`: native representation reference.
- `H = a0a0691897c6ff24985fc4db9ae51f5d77a89899`: audited preservation reference.
- `START`: actual implementation base, recorded before edits.
- Work forward from START in `integration/restore-native-btrees-memory-only`, sibling worktree `/home/simon/source/objekte-integration-restore-native-btrees-memory-only`.
- **No whole-file checkout from B** for mixed production files. Read `git show B:path`, then apply reviewed native-only hunks to START. In particular never replace `run.rs`, `lifecycle.rs`, `mutation_segment.rs`, `posting.rs`, `executor.rs` or frontend runtime wholesale.
- Do not cherry-pick all K commits over current history; their changes are already present. The K list identifies behavior that representation edits must retain.
- No B+tree work, generic storage backend, disabled disk implementation, compatibility shim, no-op load/save method, or serializer retained for tests.
- A test filtered to zero cases is not a pass. Check the reported test count. A renamed test needs an explicit old-to-new mapping.
- Do not “fix” failures by deleting query assertions, lowering exactness expectations, setting all counters to zero, marking tests ignored or replacing the oracle with the implementation being tested.
- Keep temporary implementation notes under the repository workflow. At final reporting, record every K item as verified with files/tests, or explicitly blocked; do not claim completion with an unverified carry-forward.

## 1. Decisions that are no longer left to the implementer

### D1 — Configuration identity and preferences

**Keep `configuration.rs`'s hash bytes unchanged for the same definitions/options/source registry.** Keep `CONFIGURATION_HASH_VERSION`, `MODEL_VERSION`, `BIGRAM_VERSION`, `FOLDED_REPRESENTATION`, `COLLATION_VERSION` and current feeding/order rules where used by this live identity. Correct comments to describe a live configuration identity rather than managed generation compatibility.

Rationale: `PreparedAdd`/`PreparedUpdate`, source fingerprints and desktop `SessionConfiguration` use this identity; desktop source-check preferences are keyed by it. Deleting hash inputs would silently orphan those preferences. These fixed hash-domain/semantic identifiers do not constitute an index persistence API. Delete actual segment/manifest/catalog format versions with their codecs.

Keep configuration's existing test `configuration_identity_tracks_content_inputs_and_ignores_runtime_budgets`. Add one compatibility fixture only if needed to prove that the implementation has not changed the previous hash: obtain the expected hash from START using the same definitions; do not derive both expected/actual using the new implementation. Runtime worker/cache budgets still must not change it.

Keep `RepositoryOptions` as at START except removing options whose only remaining consumer is storage. In particular do not restore `snapshot_mode`, `sync_cache`, `prepared_cache_bytes` or prepared cache defaults from B.

Keep desktop preference VERSION 2 and current lifecycle key generation. Unknown old storage/autosave fields remain ignored by serde, with all ordinary settings retained. No preference migration that clears the user's settings.

### D2 — Source discovery and old artifacts

**Delete `IndexLocationResolver` rather than extracting it into another module.** Native source opening uses `configure_definitions`, `load_repository_configuration` and existing `discovery` validation.

Important production detail: although the resolver's MemoryOnly branch leaves definitions unchanged, H's public native build methods call `IndexStorageTarget::prepare_logical_definitions`, which uses its RepositoryDirectory exclusion branch. **Do not silently remove this preparation and thereby change configuration hashes.** Extract only that source-exclusion behavior into a crate-private `repository_configuration::prepare_source_definitions(root, definitions)`:

1. Canonicalize/validate the repository root and selected source directories with the existing source error context.
2. Compute `root/.objekte/indexes` lexically; never stat/open that generated path.
3. For each source ancestor of that path, append the relative exclusion only if an existing normal-component exclusion does not already cover it. Preserve definition order, existing exclusion order and exact relative spelling as at H.
4. Reject selecting that reserved path as source. Keep generic contained-path validation in discovery.
5. Call the helper at the two existing public lifecycle-build preparation sites (`lifecycle_open.rs`, `lifecycle_initial.rs`); keep it idempotent. No targets, hashes, temp/custom base handling or storage return types in the helper.

Compare prepared definitions and configuration hashes against START for default core definitions, CLI definitions already excluding `.objekte`, and configured/multi-source definitions. This preserves existing preference identity while the traversal rule below extends skip-before-descent consistently to known generated namespaces.

- Keep CLI ordinary folder exclusions exactly: `.git`, `**/.git`, `objekte-indexes`, `**/objekte-indexes`; keep `.objekte` directory pruning and exact selected schema-file exclusion.
- Keep configured repositories' explicit include/exclude rules and schema paths. Do not mutate definition ordering or add duplicate exclusions, which would change configuration identity.
- Make known generated namespaces skip-before-descent behavior explicit in discovery if a configured source opts into hidden files: prune `.objekte` and `objekte-indexes` directory entries at each traversed level without reading their contents. Apply consistently to initial discovery and reconciliation; do not hide arbitrary `.idx`-suffixed source files.
- Do not globally reserve unrelated custom-base parent directories. The former custom base may contain ordinary source siblings; only its known `objekte-indexes` child is a generated namespace.
- Schema/config reading of `.objekte/collections` remains explicit and valid. Pruning applies to source traversal.
- A user explicitly selecting a reserved generated directory as a source root gets a source-selection error. Do not load it as an index.
- No temp-directory lookup, manifest stat, deprecated-format probe or old-store cleanup during ordinary opening. Source-root canonicalization and parent-directory enumeration remain legitimate.
- Keep source path/name context in discovery errors. Do not copy storage normalization that changes the existing contained-relative-path contract.

Test configured/manual, recursive/nonrecursive, hidden-enabled and custom-base-sibling cases. Tests must prove a poison JSON file below a generated namespace is never selected; source siblings remain selected. Permission-denied coverage must run as a nonprivileged user or be marked unavailable, not passed because root could read it.

### D3 — Readiness, error types and shutdown

Delete `IndexStorageStatus`, `IndexStorageFailureKind`, `IndexSynchronization` and their classifications. Use existing `IndexStatus`, `InitialBuildStatus`, `InitialBuildPhase` and `IndexTimingsMs`.

Do not move the two-state `IndexReadiness` enum into a new API: its consumers can use existing status:

| Old consumer | Replacement |
| --- | --- |
| storage current revision | `IndexStatus.revision` |
| complete readiness | `IndexStatus.initial_construction_complete` / task `construction_complete` |
| available catalog/partial search | task `catalogue_complete`, `searchable_batches`, existing available-record query APIs |
| source reconciliation completion | `IndexStatus.reconciliation == ReconciliationState::Complete` |
| counts | indexed_files, processed_files, live_records, fallback_records |
| loaded/saved/unsaved/durable/validation status | remove, without an always-memory dummy field |

Never equate a visible catalog or `live_records > 0` with complete results; a fully built empty repository is complete too. Preserve exact-count suppression and relation/pagination restrictions during incomplete construction.

`IndexError`: delete `InvalidCache`, `StoreConflict`, and `From<StoreReadError>`. Retain `IncompatibleConfiguration` (live prepared updates), `DestinationExists` (source move), `RevisionConflict`, `FileConflict`, `InvalidPublication`, `SourceUnavailable`, `Io`, `ResourceLimit`, cancellation and other nonstorage variants.

Delete CLI `storage_view.rs` entirely. At remaining callers use `IndexError::to_string()` and the existing frontend stage/kind mapping. An ordinary build/source error must not receive “Storage access error” or save/retry guidance.

**Keep `quiescing`, `quiesce`, `resume_after_quiesce`, `shutdown` and publication checks under the same publication lock.** They stop/reject live workers and prepared mutations. Remove sync-storage calls and flush wording from them. Quiesce cancels/waits maintenance and reconciliation; closing also cancels the frontend-owned initial-build task. Keep the current lock order; no worker join while holding the publication lock.

Keep coarse startup timing milestones and detailed preparation/install/inference timings. Delete `sealing_ns` and encoding/open/flush-specific phase fields, rather than leaving them permanently zero.

### D4 — Native ownership and internal accessor contract

Final native types:

- `PostingIndex.values: BTreeMap<ScalarKey, RoaringBitmap>`.
- `PostingIndex.lengths: BTreeMap<Number, RoaringBitmap>`.
- Structural maps/domains and presence/null/empty-selection bitmaps remain directly owned.
- `BigramIndex.normal` and `.skip`: `HashMap<u64, RoaringBitmap>`.
- `IndexedCollectionSegment.files`, `file_index`, `records`, `id_lookup`, `identity_order`: native owned data.
- `FileCatalog`: native base and bounded changed/new/path overlays; no `PersistedBase`, reader row caches or restored-generation owner.
- `FileVersions`: native chunked ownership as in B; remove persisted base/override branches introduced only for stored catalogs.

Keep the **native** `FileCatalog::snapshot_slots` and `FileCatalog::restored` operations: current direct build, base partitioning and base-containing compaction call them to rebase an in-memory catalog. Update their cache-oriented comments; these methods do not encode bytes or access a file. Delete `restored_persisted` and `PersistedBase`. Likewise, native `FileVersions::from_versions`/inventory reconstruction tests are not disk codecs merely because their names say “restored.”

Delete `sealed`, `persisted`, install/materialize/reencode methods and decoder caches. Restore the ordinary native borrowing APIs (`record`, `file`, `file_records`) already present in B. `record_count()`/`file_count()` may remain as trivial lengths.

Convert metadata calls:

| Current encoded-access call | Native expression/result |
| --- | --- |
| `try_record(slot)` | `record(slot) -> Option<&RecordEntry>` |
| `try_record_id(slot)` | borrowed ID from `record(slot)` |
| `try_record_range(slot)` | copied ByteRange from native record |
| `try_file(id)` | `file(id) -> Option<&SegmentFile>` |
| `try_file_at(n)` | `files.get(n)` |
| `try_file_entry_at(n)` | borrow `files.get(n).entry` |
| `try_file_records(id)` | `file_records(id) -> Option<&[RecordSlot]>` |
| `try_file_index(id)` | `file_index.get(&id).copied()` |
| `try_file_id_at(n)` / revision / count | native file's id/revision/records.len() |
| `try_id_lookup(id)` | native id_lookup with the existing borrowed-key conversion |
| `extend_file_slots(n, ...)` | extend from native record slots; preserve bounds checking where input is untrusted |

Remove `Cow<RecordEntry>`/`Cow<SegmentFile>` created only for persisted access. A borrowed record stays borrowed until actual result ownership is needed. Preserve out-of-domain errors at genuine validation boundaries; do not replace invalid missing metadata with empty/default data.

`cheapest_consolidation_pair` returns `Option<(usize, usize)>` again, inspecting native file/ID maps. Preserve smallest-size ordering, file/identity overlap rejection and partition record limit.

Remove `*_checked` / `try_*` dictionary wrappers whose only fallibility is byte decoding. Use B's native posting/gram operations and return shapes; update consumers together. Retain Result for query binding, invalid slot mappings, source access and resource limits. Keep K12's added native union operation as `union_id_postings(...) -> (RoaringBitmap, u64, u64)` for `(union, lists_visited, members_visited)`; remove decoder-only Result/unwrap from its callers/tests, preserving current sparse/broad selection threshold and counting rules.

Keep the 64-KiB relation scheduling guard. Cache a native `max_record_range_len` at segment finalization (and recompute for newly compacted/consolidated/remapped segments); queries must not scan every record merely to choose a scheduling mode.

`SegmentAdmission::seal` already exists as a native builder-finalization operation returning `CollectionSegment`. Rename it to `finish` and update its callers, avoiding confusion with removed byte sealing; do not delete its native assembly logic. The callers include `repository_direct.rs`, `repository_incremental.rs`, `lifecycle.rs` and native segment test fixtures.

### D5 — Native maintenance accounting

Keep current trigger constants and bounded-batch defaults:

- sparse threshold 32 MiB; size-maintenance hysteresis 16 revisions;
- minimum retirement shard size 256 records and current 25% retired rule;
- sparse admission allowance 64 plus full partitions;
- batch budget 32,768 records, 64 MiB input estimate, 128 MiB temporary estimate, 1 second elapsed;
- one oversized first shard may run to make progress, as today.

Replace encoded length with **one cached native maintenance estimate per immutable segment**, computed after its native indexes are finalized. Do not recompute every dictionary's size on each update or query.

Add a private segment accounting value with `retained_bytes_estimated` and `source_bytes_estimated`. The fields describe the same native segment; new/compacted/consolidated/remapped segments recalculate it, Arc reuse preserves it, and active-mask-only retirement retains the full physical size.

The deterministic estimate uses saturating arithmetic and counts:

1. native vector capacities times element size, including file/record/identity-order and file-slot arrays;
2. owned string/path/key payload lengths;
3. native map entries with a documented per-entry/node overhead estimate; reuse the existing gram estimate's 48-byte overhead convention where appropriate rather than claim allocator-exact size;
4. every gram/posting/structural/domain bitmap exactly once per owning index, using `serialized_size()` **only as a size proxy**, never serializing it;
5. sorting keys/orders, relation offsets/edges/positions/domains and inferred/custom posting/text families.

Do not count an encoded copy, whole repository catalog or global cache per segment. This is a planning estimate, not measured RSS or a strict memory cap. Existing `IndexFamilyEstimate` may remain the narrower family display; label its scope accurately.

Use:

```text
sparse_bytes = saturating sum(retained_bytes_estimated of sparse segments)
source_bytes_estimated = saturating sum(native file.revision.size in this shard)
input_bytes_budgeted = max(retained_bytes_estimated, source_bytes_estimated)
temporary_bytes_estimated = input_bytes_budgeted * 3 + physical_record_count * 512
```

The latter source-size charge intentionally remains conservative for a source file split across shards. Preserve report field names; update their comments to this meaning. No file read/stat is needed to calculate these values.

Size-trigger tests inject a small threshold into the existing testable helper, not huge fixtures or expected allocator byte totals. Assert nonzero estimate, threshold/hysteresis behavior, monotonic growth for additional native postings, unaffected Arc reuse and source-free compaction.

Keep current zero-record files when their catalog contribution is current. Do not drop them merely because active cardinality is zero. Keep the documented base-containing compaction limitation and verified-update latency issue open unless actual scoped evidence establishes a fix.

### D6 — Frontend replacement-open and cancellation

**Keep START's frontend state machines and remove storage branches from them.** Do not restore B's frontend files.

Desktop opening:

1. `OpenSpec` keeps `args`/`cwd`; remove `load_target`. Remove `OpenedSession.load_target` and `OpenDiagnostics.load_not_found`.
2. Rename `open_shared_storage` to `open_sources`; remove target argument, canonical custom-base resolution and `try_load` branch.
3. Keep `Index::start_incremental_build`. Incremental open returns after catalog availability; finite open waits for completion. Keep cancellation checks during waits.
4. Cancellation after obtaining an index cancels its build task and shuts down that newly built index before discarding it.
5. Keep `pending_open_generation`, opening cancellation tokens and stale-result rejection. A stale successful worker result must cancel its task and shut down its own returned session.
6. Keep the old active workspace usable until replacement succeeds. A failed/cancelled source open leaves it active. Only after successful replacement cancel/stop old session tasks and release it.
7. Rename `open_after_flushing_previous` to `open_replacement`; remove unused previous/check-policy arguments, retain its post-open cancellation cleanup.
8. Remove `storage_sync_operation_id`, `close_after_storage_sync`, save-task state, load/save/change-storage actions and worker events. Preserve independent check/rebuild/compact task states and operation IDs.
9. `request_close` invokes ordinary shutdown directly. Rename `close_without_saving` to `start_close`. Keep close failure/cancel-resume handling where it protects live lifecycle state; remove save-wait/retry branches and storage dialogs.

TUI: retain opening/loading channel, task cancellation, query/completion generations, source checks, rebuild/compact and shutdown. Remove only storage menu/selector/action/event state. Fix focus, menu indices and shortcuts after deletions.

CLI: always use current `open_incremental_repository`; preserve early query preview and final ordered protocol. Delete `open_saved_repository`, target resolution, flush report and index-only invocations. Require a normal query/query-file/FTS/TUI input as appropriate.

### D7 — Output schema and runnable benchmarks

Change CLI `PROTOCOL_VERSION` from 3 to **4**, with a comment that v4 removes persisted-index events/fields. Update desktop batch consumers, process fixtures and docs in the same checkpoint.

Remove load/flush/storage-action, saved-origin/destination/durable/validation fields and index-load/index-flush error stages. Keep run/query event order, sequence numbers, exit status, query statistics, source errors, millisecond formatting and cancellation. Remove prepared-cache hit/budget fields if a baseline hunk attempts to restore them.

Keep `bincode` for `verified.rs`'s in-memory query key and remaining scalar-validation tests; keep `sha2` for source/live identity. Remove `memmap2`. Remove serialization-only derives/features from index objects once all legitimate consumers are checked; do not remove source/protocol/preferences serde support.

Delete runnable `bench_managed_flush.rs`, `bench_managed_open_alloc.rs`; keep already-deleted `bench_snapshot_save.rs` absent. All other benchmark examples listed below keep their native build/query/update workloads, drop storage/cache selectors and use START's profiling/provenance.

Keep historical results, including commands/scripts inside an archive, as frozen evidence. Clearly mark archived storage campaigns retired; active benchmark entry points must not call them.

## 2. Per-file production instructions

Paths in the first table are relative to `crates/core/src/`. Start from START in every row. B is a function/type reference, not a file replacement command.

| Files | Required edit / retained behavior |
| --- | --- |
| `posting.rs` | Native maps/ranges/borrowed bitmaps; remove sealed adapters; keep values_indexed, coverage flags, structural facts for all declared fields, guarded self-comparison and broad ID union K08–K13. |
| `bigram.rs` | Native normal/skip maps and pending construction buffers; delete sealed reader/cache/presence filter; retain one-pass partition projection and source text folding. |
| `index_catalog.rs` | Native posting/gram access and estimates; preserve declared array routing and exactness. |
| `executor.rs` | Replace decoder calls with native borrowed metadata; preserve field-state/self-comparison, metadata eligibility guards and direct timestamp comparisons. |
| `posting_values.rs` | Native bounded ordered traversal; preserve selection identity/revision, reverse/prefix paging, inactive filtering, cursor bounds and source-backed verification where needed. |
| `run.rs` | Surgical accessor changes only; keep fresh binding, SmallVec ranges, grouping fallback, lazy evidence, sparse relation scans, page admission, partial-view rules, caches, EXPLAIN and counters. |
| `relation_query.rs`, `relation_semijoin.rs` | Native candidate/metadata access; retain parallel bounds, missing-target diagnostics, cancellation, target cache and distinct list/member counts. |
| `relation.rs` | Preserve one-pass partition scattering and occurrence positions/domains; delete codec-only visibility/helpers/derives if unused. |
| `sorting.rs` | Preserve inferred-order cache, one-pass partition sort projection and resource limits; replace persisted metadata access only. |
| `sort_binding.rs` | Revert only Cow-driven extra-reference adaptation where native borrowed record requires it; preserve binding behavior. |
| `inference.rs`, `inference_build.rs` | Native posting construction; keep structural facts after admission failure, values_indexed and collection-wide inference reports; remove sealed field/codec access. |
| `build.rs` | Keep inference_copy and borrowed logical_records, preparation/admission measurements; publish native assembled objects and rename native seal to finish. |
| `repository_incremental.rs` | Keep global inference observation/merge and borrowed logical domain, separate inference timing and progressive publication; remove sealing time/encode phase. |
| `repository_direct.rs` | Publish directly from native completed data; keep direct-source semantics and current diagnostics. |
| `repository.rs` | Keep current snapshot-free public constructors/options and binding-cache removal; remove stored-generation owner constructors and codec imports. |
| `prepared.rs`, `reference.rs` | Keep fresh preparation and binding logic from START, plus path/self-comparison eligibility; no PreparedCache or retained binding-size accounting. |
| `regex_query.rs` | Remove checked sealed-gram path; use native candidates; keep deleted prepared-cache retained_heap_bytes deleted and keep legitimate regex limits. |
| `source_scan.rs` | Production scanner unchanged; preserve snapshot-option removal from its inline tests. |
| `measurement.rs` | Keep profiling gates and reverse-member counters; no prepared_plan_hit or decoded-cache metrics. |
| `configuration.rs` | D1 byte-compatible live hash; comment changes only unless START differs. |
| `repository_configuration.rs` | Keep current source/schema loading and already-removed protected_files; extract only the D2 source-exclusion helper, with no cache-aware repository loader. |
| `inventory.rs` | Restore native FileVersions chunks and direct lookup; retain all normal file/source fingerprints/revisions/identity. |
| `lifecycle_catalog.rs` | Remove PersistedBase/catalog decoder dispatch and row caches; retain native Arc base and changed/new/path overlay locality, active file membership and pinned views. |
| `mutation_segment.rs` | Remove persisted field/accessors; keep all K19–K22 projection structs/methods, consolidate/compact/remap invariants; compute D4/D5 accounting on every new finalized native segment. |
| `lifecycle_partition.rs` | Keep bounded base partitioning, shared routing and one-pass projection; native file/record references and no seal stage. |
| `lifecycle_compaction.rs` | Preserve START's Arc reuse, bounded loop, cancellation, CAS retry and reporting; D5 replaces source_budget_bytes encoded branch. |
| `lifecycle_verified.rs` | Preserve slot-preserving/reconciliation fallback strategy and bounded scope; native metadata access only; no disk hooks. |
| `lifecycle.rs` | D3/D5; remove storage fields/methods listed below; retain live publication, cache ownership, maintenance, source check and quiescence. |
| `lifecycle_open.rs` | Keep explicit build/build_with_sources and direct construction; replace logical-definition preparation with D2 helper; remove owned sealing and storage hooks. |
| `lifecycle_initial.rs` | Keep task cursor, catalog/progress/wait/cancel semantics; replace definition preparation with D2 helper; remove owned sealing and restore-only `from_ready` if it has no native caller. |
| `lifecycle_caller.rs` | Keep pinned compatibility metadata/queries; remove persistence wrappers and saved-status forwarding. |
| `lib.rs` | Remove deleted-module exports; keep current surviving APIs and live MODEL_VERSION as D1. |
| `discovery.rs` | D2 pruning/diagnostics if required; keep current include/hidden/symlink/source-reader semantics. This file is unchanged in B..H but may need the explicitly scoped edit. |

In `lifecycle.rs`, delete:

- `IndexInner.restored_from_storage`, `seal_owned_memory`, `managed`, `loaded_origin`, `storage_status_snapshot`, `storage_syncing`, test-only `sync_storage`, `StorageSyncActivity`, `SyncCacheWorker`;
- `PublishedIndex.saved_revision` and `IndexStatus.saved_revision`;
- load/flush/attach/origin/status/sync-storage methods, hooks and helper types; decoded posting-cache usage API;
- sealing-specific arguments/branches of initial publication, maintenance and rebuild.

Keep the remaining fields, notably `verified_cache`, `timings`, `published`, writer/publication locking, maintenance state, periodic reconciliation and quiescing. Fold `make_room_for_segment_sealing` into `make_room_for_segment`; remove `seal_owned` arguments from the limits helper, preserve the full current trigger algorithm.

Delete whole modules **and companion `.test.rs` files**:

```text
bigram_dictionary
generation_catalog
hybrid_segment_codec
index_storage
lifecycle_storage
managed_store
ordered_block
posting_dictionary
segment_format
segment_metadata
```

Keep absent: `storage`, `lifecycle_cache_guard`, `lifecycle_repository`, `lifecycle_segment_codec`, their tests, `prepared.test.rs`'s removed cache tests. Delete `test_support.rs` if its only content remains managed roundtrips (true at H), and remove its `mod` declaration.

Frontend table paths are relative to `crates/`.

| Files | Required edit |
| --- | --- |
| `cli/src/arguments.rs` | Delete all historical/current load/save/flush/storage/residency fields, aliases, target/path resolvers and index-only validation exemptions; preserve ordinary source/query options. |
| `cli/src/loader.rs` | Keep configure_definitions/repository_options and source/schema exclusions; remove storage test adapters/imports; use already-loaded definitions without rereading schemas. |
| `cli/src/execution.rs` | D6: only progressive fresh-source open; delete save/load branches and reports; keep query-input handling, previews, cancellation, exit status and event sequencing. |
| `cli/src/output.rs`, `desktop/src/backend/batch.rs` | D7 protocol 4 and field removal; retain payload compaction, independent errors and exports. |
| `cli/src/lib.rs`, `cli/src/storage_view.rs` | Remove module export and delete storage_view; use ordinary error Display. |
| `cli/src/tui/app.rs`, `tui/mod.rs`, `tui/view/mod.rs`, `tui/test_support.rs` | Remove storage enum/menu/worker/dialog/status state and cases; preserve loading, available queries, completion, source-check/maintenance and shutdown paths. |
| `desktop/src/backend/open.rs` | D6 native opening/cancellation; retain schema literals and configuration identity. |
| `desktop/src/backend/index_lifecycle.rs` | Delete load/save/destination/status wrappers; keep status/check/rebuild/compact/quiesce/shutdown/resume functions. |
| `desktop/src/backend/test_support.rs` | Native source fixtures; remove OpenSpec.load_target initializers. |
| `desktop/src/controller/adapt.rs` | Build source-only Arguments/OpenSpec; retain collection/root/schema/query conversion and validation. |
| `desktop/src/controller/open_detection.rs` | Keep source root/schema detection; no saved-index discovery. |
| `desktop/src/controller/runtime.rs` | D6; live Index panel uses D3; source-keyed preferences and current session identity remain. |
| `desktop/src/ui/model.rs`, `ui/mod.rs`, `ui/state.rs` | Delete storage choices/actions/updates/task state; preserve check/rebuild/compact and ordinary close/open state. |
| `desktop/src/ui/open_dialog.rs` | Remove load-saved checkbox/storage/destination modes/pickers; keep source folder/multi-folder/repo/schema selection and stale-picker guards. |
| `desktop/src/ui/panels.rs`, `ui/render.rs` | Delete storage buttons, headings, errors and labels; show live readiness/counts/freshness and scoped memory estimates. |
| `desktop/src/ui/preferences.rs` | D1; retain source-check settings, history/bookmarks/layout/theme; no autosave restoration. |

Other production/runnable paths:

- `crates/core/Cargo.toml`, `Cargo.lock`: remove memmap2 and storage-only features after consumer audit; retain bincode/sha2 uses above.
- Surviving examples: `accept_musenalm`, `bench_build_musenalm`, `bench_candidate_verification`, `bench_case_modes`, `bench_corrective_cases`, `bench_index_construction`, `bench_inference_fixture`, `bench_initial_concurrency`, `bench_lifecycle_mutations`, `bench_mixed_planner`, `bench_musenalm`, `bench_optional_features`, `bench_predicate_optimization`, `bench_query_features`, `musenalm_without_schema`, `verify_structural_reference` under `crates/core/examples/`: remove storage/prepared-cache controls, keep native workload and profiling/provenance. Fix `examples/support/musenalm.test.rs`.
- `bench/run_build_regressions.py`, `bench/run_incremental_pairs.py`, `bench/run_optional_features.py`: update current invocations/result schema to source builds only. Preserve paired order, binary hashes, fixture/corpus identity and native work metrics.
- Current docs: `bench/README.md`, `docs/cli/obj.md`, `docs/tui.md`, `docs/search-core.md`, `docs/development/gui_environment.md`, `docs/development/index-lifecycle-contracts.md`, `docs/future-whole-file-replacement.md`: replace persistence instructions with final source-build/live lifecycle behavior.
- Retire `docs/development/hybrid_index_storage.md` and `direct_snapshot_{access,catalog,segment}_format.md`: replace each current body with a short retirement notice linking `docs/search-core.md` and the corresponding historical task specification in `docs/ai/done/`. Retain the paths so existing links resolve, but no current format/load/save instructions. The original content remains in Git history and completed task records.
- `docs/ai/done/**`, `docs/ai/generated/**`, `bench/results/**`: preserve historical records. Record obsolete persistence issues in completion report; retain unrelated compaction/update-latency issues and previous issue moves.

All changed production files in B..H are assigned above. Verify the list again against START; an unassigned later path needs a disposition before editing.

## 3. Test migration, without weakening coverage

### General fixture transformation

Start with current assertions. Remove only persistence setup and serialization-specific assertions.

| Existing fixture | Replacement |
| --- | --- |
| repository snapshot write/read | `Repository::open_with_options` from source definitions, same options/registry |
| `Index::open_with_storage(... MemoryOnly ...)` | `Index::build_with_sources` |
| managed reopen for static-vs-lifecycle parity | fresh static Repository versus native Index, plus incremental Index when testing segmented execution |
| managed restore after mutations | continue querying the same mutated live Index; compare with an independent fresh build from current sources where possible |
| restored old view after source deletion | build first, pin native view, then remove source; test the same source-free query/maintenance assertions |
| reopened cache is empty | construct a second native index; verify instance/cache isolation and identical answers |
| persisted catalog pinned across mutation | pin native catalog/view before mutation; assert old/new identities and results independently |

Do not rebuild from source **after** deleting or corrupting it merely to replace a saved-index load. Do not replace a mutated index with a fresh one if the test exercises retired slots, historical versions, moved-file handles or pinned view identity.

Delete `crates/core/tests/support/legacy_storage.rs` and imports. No replacement trait with storage-shaped names.

Apply these migrations to all changed core tests: `bigram`, `build`, `dynamic_filter`, `executor`, `folder`, `foundation_invariants`, `id`, `inference`, `lifecycle`, `lifecycle_caller`, `lifecycle_catalog`, `lifecycle_compaction`, `lifecycle_initial`, `lifecycle_mutation`, `lifecycle_open`, `lifecycle_partition`, `lifecycle_scan`, `mutation_segment`, `posting`, `posting_values`, `query`, `reference`, `regex_query`, `relation`, `relation_inspection`, `relation_query`, `relation_semijoin`, `relation_target_cache`, `repository`, `repository_configuration`, `run`, `sort_binding`, `sorting`, `source`, `streaming` (`crates/core/src/<name>.test.rs`), and inline tests in `source_scan.rs`.

Integration tests to retain with native fixtures: `array_positions`, `g2_interactions`, `inline_sort`, `non_json_relations`, `optional_feature_interactions`, `parameters`, `regex_record_id`, `relation_occurrence_oracle`, `relation_oracle`, `temporal_periods`, `temporal_regex_integration` in `crates/core/tests/`.

Client tests: update each changed `.test.rs` beside the frontend production files above; also preserve desktop backend `export.test.rs` and `query.test.rs`, and CLI `tests/obj_process.rs`/`obj_removed_flags.rs`. Delete save/load-only tests; retain failure/cancel/replace/source/protocol assertions using source-opening failures instead.

### Specific required migrations

| Existing name | Required final name/meaning |
| --- | --- |
| `compacting_sealed_segments_preserves_text_postings` | `compacting_native_segments_preserves_text_postings`; native FTS result parity before/after source-free compaction |
| `compaction_reuses_unaffected_sealed_segment` | `compaction_reuses_unaffected_native_segment`; retain actual `Arc::ptr_eq` locality assertion |
| `compacted_base_contributions_round_trip_through_managed_store` | `compacted_base_contributions_preserve_native_queries`; preserve base/retired contribution assertions on live compacted root |
| `partition_and_managed_restore_keep_global_inference_diagnostics` | `partition_and_native_publication_keep_global_inference_diagnostics`; compare global report across partition/publication |
| `partial_structural_postings_survive_explicit_mutations_and_cache_roundtrip` | `partial_structural_postings_survive_explicit_mutations_and_compaction`; preserve mutations, completeness and query assertions |
| `inferred_order_cache_reuses_pages_and_restores_without_persisting_orders` | `inferred_order_cache_reuses_pages_and_is_scoped_to_native_instance`; preserve reuse, answer parity and independent fresh owner |
| `cache_exclusion_precedes_seen_tracking_and_scan_cancels_between_batches` | `generated_directory_exclusion_precedes_seen_tracking_and_scan_cancels_between_batches`; preserve exclusion/seen/cancellation semantics |
| `failed_saved_index_open_keeps_previous_memory_session` | `failed_source_open_keeps_previous_session`; fail source/schema opening, assert same retained session |
| `configured_memory_open_ignores_existing_deprecated_snapshot` | `source_open_ignores_existing_index_artifacts`; retain poison artifact and successful source query |

Delete `sealed_broad_id_union_uses_checked_borrowed_keys`; its native counterpart remains. Codec corruption/layout/format/locking/stamp/flush-retry tests disappear with their modules. Move any semantic-only assertions not already covered into native tests before deleting the codec fixtures.

### Named regression matrix

Names below exist at H unless explicitly marked NEW or renamed above. Run relevant groups at checkpoints and the full workspace at final acceptance.

| Group / K requirements | Exact existing tests and acceptance |
| --- | --- |
| T1 native structural/ranges; K08–K10 | `declared_structural_facts_are_exact_without_a_value_dictionary`; `indexed_field_state_matches_independent_source_oracle_without_verification`; `narrow_ordered_ranges_visit_only_matching_dictionary_keys`; `structural_membership_tracks_file_replacement_move_delete_and_add`; `segmented_array_step_state_predicates_use_exact_structural_postings`. Last test also checks self-comparison/raw presence, inequality fallback and segmented EXPLAIN. |
| T2 scanner/page work; K11/K16 | `segment_file_grouping_merges_out_of_order_file_runs`; `lifecycle_fts_parallel_files_match_single_worker_page_and_counters`; `lifecycle_parallel_residual_filters_match_single_worker_and_reference`; `lifecycle_exact_sorted_pages_use_stored_order_with_full_count`; `exact_posting_count_preserves_page_evidence_with_and_without_membership_cache`. Retain evidence and work assertions. |
| T3 relations/diagnostics; K12–K17 | `broad_id_union_matches_individual_lookups`; `both_physical_strategies_keep_independent_witnesses_and_reuse_targets`; `lifecycle_parallel_relation_files_preserve_answers_and_missing_target_diagnostics`; `lifecycle_parallel_direct_targets_match_serial_and_unavailable_fallback`; `segmented_selected_record_id_regex_uses_metadata_without_source_reads`. Preserve separate list/member counts and zero-source-read assertions. |
| T4 partition/locality; K18–K24 | `compaction_matches_active_file_oracle_across_indexes`; `historical_file_versions_compact_to_current_zero_record_or_delete`; `compaction_remaps_retained_fallback_diagnostics`; `full_partitions_leave_a_bounded_sparse_update_allowance`; `partitions_static_base_without_changing_search_or_file_routing`; `compacts_one_large_file_in_source_free_parts`; renamed native/inference tests above. |
| T5 maintenance/cancel; K23–K25 | `sparse_byte_trigger_coalesces_with_revision_hysteresis`; `retired_ratio_trigger_compacts_one_affected_shard`; `bounded_compaction_can_resume_remaining_retired_shards`; `background_compaction_preserves_pinned_reader_and_reports_completion`; `large_source_free_compaction_cancels_and_retries_a_concurrent_commit`; `segment_admission_keeps_large_partially_retired_shard_when_small_pairs_fit`; `shared_partition_verified_update_preserves_its_record_slot`. |
| T6 lifecycle/close; K25 | `background_rebuild_waits_and_cancelled_preparation_never_publishes`; `stopping_periodic_schedule_does_not_cancel_current_reconciliation`; `quiescence_keeps_unavailable_lookup_without_reconciliation`; `quiescence_rejects_prepared_publications_until_close_is_cancelled`; current lifecycle_initial suite. |
| T7 clients/config; K03/K25–K27 | `configuration_identity_tracks_content_inputs_and_ignores_runtime_budgets`; `configured_open_uses_the_already_loaded_schema`; `manual_schema_still_ignores_repository_metadata`; `cancelled_replacement_open_invalidates_late_loader_without_losing_active_draft`; `replacement_open_retains_old_workspace_until_success`; `lifecycle_preferences_are_off_by_default_and_scoped_to_source`; `loading_screen_shows_index_progress_before_repository_is_ready`; renamed failure-open test above. |
| T8 binding/cache boundaries; K01/K02 | `crates/core/tests/parameters.rs`, `regex_record_id.rs`, `temporal_regex_integration.rs`; `cancellation_and_sink_failure_do_not_publish_verified_membership`; `inferred_order_cache_reuses_keys_and_evicts_least_recently_used`; renamed per-instance inferred-order test. Audit fresh prepare calls and absence of binding cache symbols; profiling run records fresh binding every request. |
| T9 build; K05/K06/K07 | `one_large_framed_file_uses_parallel_ranges_with_identical_indexes`; `auto_index_retains_structural_facts_after_value_admission_fails`; `multibyte_distinct_boundary_and_partitioned_merge_agree`; construction benchmarks, clean check and profiling build. Inspect logical_records borrowing: output parity alone cannot prove no bitmap clone. |

Necessary focused gaps (NEW acceptance cases; extend an existing test when natural):

- **Metadata fast-path guard parity:** supported direct timestamp comparisons and unsupported transforms/period/matcher combinations match independent reference in static and segmented modes; no source reads for supported metadata-only queries.
- **Native range scheduling boundary:** candidate ranges at 65,536 and 65,537 bytes choose the permitted path without dropping/duplicating results; single-/multi-worker equality and cancellation.
- **Native maintenance estimates:** nonzero for an ordinary native shard, grow with additional postings, retain full retired-slot ownership and trigger testable hysteresis without source reads.
- **Source exclusion/no-storage:** D2 fixtures for hidden configured discovery/reconciliation and ordinary CLI process opening.
- **Removed flags/actions/protocol:** every removed flag fails with argument error; supported protocol-4 query succeeds; menus/actions contain no save/load/retry index operation; old preference unknown fields preserve normal settings.

K28 is verified across migrated suites. K29 is a diff audit: historical records preserved, active docs/current commands updated, unrelated issue state retained.

## 4. Ordered compilation checkpoints

Do not promise each file edit compiles. The native representation cut has dependent consumers; apply its bounded set as one working change and verify its completion. Never introduce dummy implementations to make an intermediate build green.

### C0 — Freeze and baseline

1. Record START and working state; read any new instructions in the task worktree.
2. Verify B/H ancestry. Extend ledger over H..START and inspect merge resolutions if nonempty.
3. Record source corpus, relevant existing test counts and runnable benchmark commands; capture START's fresh-memory query/build/update results before removing selectors.
4. Record configuration fixture hash and protocol shape for compatibility/migration tests.
5. Save execution checklist K01–K29/T1–T9, and baseline limitations. No implementation claim yet.

Gate: complete delta ledger, known reproducible baseline commands and available datasets/tooling. A missing external benchmark dataset is reported explicitly and does not justify inventing passing results.

### C1 — Remove client persistence and storage-backed semantic fixtures

This checkpoint still uses START's encoded memory backend internally; it is only caller decoupling.

1. Update core semantic test fixtures to native **public memory entry points**, preserving assertions; migrate integration helpers and remove storage-shaped test bridges once no callers remain.
2. Make CLI/TUI/desktop use the existing fresh source builder only. Apply D1/D2/D3 frontend status, D6 cancellation and D7 protocol together.
3. Remove persistence controls, client storage tests and runnable disk benchmarks. Keep core codec/store modules and their own tests until C2.
4. Update surviving examples to memory-only entry points and remove prepared-cache controls.

Gate: workspace tests/check, T6/T7/T8 and supported/removed CLI process cases pass. Current core storage exports may still exist solely pending C2; no frontend or surviving nonstorage example calls them.

### C2 — Atomic native core conversion and persistence deletion

Work in this order within the checkpoint:

1. Restore native posting/gram implementations while carrying K08–K13, K20 and K22 into the edited definitions.
2. Restore native segment/catalog/version ownership; preserve partition routing, one-pass projections, pinned views and native max-range/accounting.
3. Convert query/value/sort/relation consumers using D4, preserving K08–K17. Retain START's fresh binding and remaining caches throughout.
4. Convert build/initial publication/verified updates/maintenance with K05/K06/K18–K24 and D5; remove every encode/seal/materialize path.
5. Delete storage modules/exports/APIs/state and their tests. Remove obsolete error variants and memmap2; preserve live configuration hashing and shutdown.
6. Migrate any remaining semantic fixtures that inspect native internals; remove serialization-only fixtures/derives after checking consumers.

Gate commands:

```sh
cargo check -p objekte-search-core --all-targets --locked
cargo test -p objekte-search-core --locked -- --quiet
cargo check --workspace --all-targets --locked
cargo test --workspace --locked -- --quiet
```

Run T1–T9 filters when diagnosing; full suite already covers passing filters. Confirm the deletion audit below has no executable storage implementation. No final checkpoint with a disabled backend or compatibility trait.

### C3 — Regression gaps, docs and interface audit

1. Complete only meaningful uncovered NEW cases above and protocol/preference migrations.
2. Review diff against START for lost unrelated behavior and against B for unintended nonnative dependencies.
3. Match every changed production path and every K row to evidence. Check no old defaults/cache/API have been revived.
4. Update active docs, examples and scripts; retire obsolete active format docs, preserve history.
5. Run final checks after these edits:

```sh
cargo test --workspace --locked -- --quiet
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
git diff --check
```

Use repository formatting rules and record pre-existing drift rather than reformat unrelated files. Repeat tests only after relevant changes/failures; C2 results remain evidence if code has not changed.

### C4 — Query/performance, actual frontends and no-index-I/O

1. Five-repetition extended 220-case ASCII and 220-case Unicode campaign with verified-result reuse disabled; compare START versus candidate counts, ordered results and evidence/digests.
2. Independent jq oracle (150 predicates in each alphabet) and existing reference evaluator. The 150-case runner is **not** a substitute for the extended 220-case campaign.
3. Matched release fresh-build/incremental/update/compaction/pinned-view measurements with identical corpus/options/workers; record peak RSS, retention and timing distributions. Use main-plan named benchmark examples and current archived command provenance, deleting only storage arguments for the candidate.
4. Actual CLI, PTY TUI and native GUI source open/progressive query/completion/source-check/rebuild/cancel/replace/close. Follow `docs/development/gui_environment.md`; screenshots/logs should show storage controls gone and remaining actions functional.
5. Filesystem inventory plus focused tracing of all open/read/write/rename/unlink activity during source open, query, mutation, compaction and shutdown. Seed absent/malformed/inaccessible old artifacts in disposable fixtures. Compare logical outcomes and artifact bytes/metadata before and after.

Tracing distinguishes application index I/O from source reads, shared-library/runtime files, normal preferences and requested result/benchmark outputs. Check canonical/symlink aliases too. No opens beneath old generated index roots, no snapshot probes, manifests, `.idx` spill or index cleanup. Parent directory enumeration alone is not an index-file read.

No new percentage performance budget. Investigate material paired regressions; existing base-compaction and verified-update issues must not be represented as newly solved on architecture alone.

#### Concrete extended query invocation

Build separate ordinary release `bench_musenalm` binaries at START and candidate, retaining their hashes and build revisions. Set `BASE_BENCH`, `CANDIDATE_BENCH`, `CORPUS` and `RESULTS` to their actual absolute paths; the two result subdirectories must not already exist. Use the same immutable corpus and environment for both:

```sh
"$BASE_BENCH" --root "$CORPUS" --extended --incremental \
  --profiles ascii-off,unicode-off --verified-cache-bytes 0 \
  --query-workers 32 --repetitions 5 --storage memory \
  --result-dir "$RESULTS/baseline"
"$CANDIDATE_BENCH" --root "$CORPUS" --extended --incremental \
  --profiles ascii-off,unicode-off --verified-cache-bytes 0 \
  --query-workers 32 --repetitions 5 \
  --result-dir "$RESULTS/candidate"
```

These options are present in H's example; candidate removes only `--storage memory`. If START's CLI has changed, record the equivalently matched invocation during C0. Compare the full 440 `(case_id, profile)` key set in `query-results.csv`, `result-digests.json`, statuses/counts/pages/evidence, and five repetitions' timing data; an empty diff from a subset is not sufficient.

Command provenance: `bench/results/remove-binding-cache/final-20261114/campaign-summary.json` and `bench/results/remove-binding-cache/scripts/compare_campaign.py`. **Do not run that archived comparison script unmodified**: it primes disk stores, injects the removed prepared-cache flag for its old baseline and also runs on-demand mode. Reuse its result comparison logic in a task-local harness or manually compare its output files; leave the archived script unchanged.

The historical corpus path was `/home/simon/source/objekte-integration-hybrid-index-storage/examples/musenalm` (148,486 source records). Verify availability and identity rather than assuming a historical worktree still exists. Mutations use disposable copies, not this canonical corpus.

Run the independent source oracle with the current documented entry point:

```sh
python3 bench/verify_jq_oracle.py --result-dir "$RESULTS/jq-oracle"
python3 bench/run_build_regressions.py --result-dir "$RESULTS/build"
```

Record the scripts' actual resolved corpus and commands. Compare build results with a START baseline recorded by the same script under matched settings. Use current native scenarios from `bench_lifecycle_mutations` and `bench_initial_concurrency` for update/compaction/readiness, preserving their existing fixture isolation.

### C5 — Final evidence and delivery

The completion report contains:

- START/B/H, branch/worktree and pushed implementation commits;
- K01–K29 evidence map; native type/accessor ownership review; all ledger dispositions including later delta;
- removed public APIs/modules/flags/actions, protocol 4 and unchanged live configuration identity;
- migrated/deleted tests with old-to-new mapping, actual commands/counts/outcomes;
- query digests, paired build/query/update/memory measurements and exact benchmark invocation/provenance;
- actual frontend and no-index-I/O evidence;
- preserved historical records, active documentation changes, known limitations and blockers;
- artifact paths observed for optional future manual cleanup; no automatic deletion shipped.

Follow repository report/move/commit/push rules on the integration branch. No merge to master.

## 5. Final negative and positive audits

Search **current executable sources/tests/examples/manifests and active docs**, not frozen history, for:

```text
IndexStorage  IndexResidency  IndexLocationResolver  IndexFlushReport
ManagedStore  ManagedPersistence  RestoredGenerationCatalog
SegmentReader  SegmentWriter  SealedPosting  SealedBigram  PersistedRecordSlots
try_load  flush_to  flush_storage  attach_storage  storage_status
open_with_storage  restored_from_storage  loaded_origin  saved_revision
save_snapshot  load_snapshot  SnapshotMode  SnapshotOpenPolicy
flush_cache  load_cache  sync_cache  seal_owned_memory  seal_segment
DecodedPosting  decoded_cache  PreparedCache  prepared_cache_bytes
prepared_plan_hit  prepared_cache_hit  memmap2
--load-index  --flush-index  --write-index  --index-storage
--index-directory  --index-residency
```

Substrings are review aids, not an indiscriminate deletion script. Negative flag tests and retirement documentation deliberately name removed features. Classify each remaining hit with a file/line reason. Generic “snapshot” for in-process InitialBuildSnapshot/native catalog slot rebasing, native `generation`, source caches, and bitmap `serialized_size()` are not disk persistence. Conversely, renaming a codec to avoid a grep does not meet the requirement.

Positive audit:

- completed maps populated and borrowed by static/lifecycle query paths;
- no whole-index serialized byte buffer, mmap owner, reader dispatch or decoded cache;
- prepared binding occurs freshly, while verified/inferred-order/relation-target caches retain budgets and owner/revision checks;
- native construction/partition/compaction does not read sources beyond the existing source build/update operations;
- no `logical_records().clone()` reintroduced in incremental inference accounting;
- `values_indexed`, structural coverage, raw-presence self-comparison and one-pass projection all present;
- source-check, maintenance and cancellation still callable from clients;
- source/schema files and ordinary settings/exports retain their intended behavior.

When a check fails, repair the affected step and rerun its relevant gate. Do not mark the task complete merely because the application compiles.
