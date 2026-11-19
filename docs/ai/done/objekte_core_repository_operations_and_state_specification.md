# Assignment 7 — Repository operations and state

**Task** `core-repository-operations-and-state`  
**Integration branch** `integration/core-repository-operations-and-state`  
**Worktree** `/home/simon/source/objekte-integration-core-repository-operations-and-state`  
**Reviewed prerequisite** `04a057b74e37e4c13df1736edca9214a736b58f2`  
**Purpose** Complete the last row of the core module roadmap. A maintained Rust caller must be able to configure a collection, construct an index, pin a view, execute a query and inspect its results using the defining public modules. The work is a source-ownership and import migration inside `objekte-search-core`.

## 1. Establish the starting revision

Read `AGENTS.md` and `docs/ai/README.md`. Follow the task-document and worktree workflow there when the owner assigns implementation. Fetch the current refs and record the full SHAs. Use merged `origin/master` when it contains the reviewed prerequisite. If implementation starts before that merge, branch directly from the exact prerequisite SHA. Do not start from the older `601a8e59` master. Record the actual base as **B7** and use it for all comparisons. If newer master already changes a source named below, inspect that diff and preserve its behavior. Keep all delivery commits on the integration branch.

On B7, capture the default and all-feature core unit inventories using `cargo test --locked -p objekte-search-core --lib [--all-features] -- --list` (run once with and once without the option). Capture the names of the test targets in the three workspace crates. Count direct Rust files with `find crates/core/src -maxdepth 1 -name '*.rs' -type f | sort`. Record a symbol-to-file inventory for the three production roots below. Search the workspace for direct, grouped, aliased and fully qualified imports, including examples, doctests and desktop callers. Save the inventories as working evidence for the completion report.

## 2. Canonical ownership

Paths in the table are relative to `crates/core/src/`. Each public type has one defining public path. The method receiver and existing method signatures remain the public operation interface. Preserve struct field visibility, derives, lifetimes, variants and documented errors. Preserve the privacy boundary on published state, including the compile-fail example on `Repository`.

| B7 source or symbol | Destination and decision |
| --- | --- |
| `repository.rs` resident `Repository`, its instance counter and allocator | Define `Repository` in `repository/mod.rs`. Its public name stays `repository::Repository`. Keep one process-wide instance counter. Declare the child modules here, with a short module-level example or a link to the public caller test. |
| `IdentityPolicy`, `CollectionDefinition`, `CollectionDefinition::folder`, `RepositoryOptions` and its impls | Define in `repository/configuration/mod.rs`. Use `repository::configuration::{CollectionDefinition, IdentityPolicy, RepositoryOptions}`. Preserve folder defaults for JSON versus JSONL/registered readers, recursive discovery, sorting, worker defaults, inference and cache budgets. |
| Root `repository_configuration.rs` and companion | Move to `repository/configuration/loader.rs` and `repository/configuration/loader/tests.rs`. Define `RepositoryConfiguration`, `ExplicitSchemaFile`, `load_repository_configuration` and `load_explicit_schema` under `repository::configuration::loader`. Keep source preparation, path normalization, generated-namespace exclusions and protected-file handling together. Retain existing internal visibility. Schema compilation stays in `schema::json`. |
| Root `configuration.rs` and its inline tests | Move to `repository/configuration/identity.rs`, including `configuration_hash`. Its public path is `repository::configuration::identity::configuration_hash`. Retain version 17 on the reviewed tree, byte representation and order, normalization, domain tag and golden fixture. On a later B7 preserve its version. A path move requires no hash or on-disk format change. |
| `CollectionState`, `GlobalInferenceSummary`, `CollectionStats` and its methods | Define in `repository/state.rs`; keep the internal summary crate-private. This is the assembled state consumed by construction, lifecycle and query code. Move the definitions verbatim. Preserve existing borrowed accessors, collection order, slot identity, prepared reader, active masks and shared segment ownership. |
| `StartupStats`, `BuildProfileStats`, `IncrementalSegmentProfile`, `BuildContributionStorageProfile`, their impls and the profiling RSS helper | Define in `repository/statistics.rs`, preserving all profiling feature gates and zero-cost defaults. Keep counting and sampling at their current call sites. |
| Public query record types at `repository.rs:578–609` | Define `SearchHit`, `Payload` and `SearchResult` directly in the existing `query_results/mod.rs`. Use `query_results::{Payload, SearchHit, SearchResult}` everywhere. Keep the existing private payload decoding and materialization modules. Preserve all fields, variant payloads and comparisons. Keep `WorkCounters` in `measurement`, `ReturnMode` in `execution_options`, streaming events in `streaming`, and the construction-completeness wrapper `AvailableSearchResult` in `lifecycle`. |
| Repository operations currently in the long `impl Repository` | Use private `repository/records.rs` for record lookup, hit generation stamping and diagnostic remapping. Use private `repository/inspection.rs` for inventory, collection, relation, inference, cache-budget, segment, signature and statistics accessors, including the private signature hash feeder. Use private `repository/search.rs` for optimized and reference search entry points, count, parameter/context variants, explain and streaming dispatch. Place whole-result stamping helpers beside hit stamping. Keep worker creation, cache configuration and `empty_like` with the resident owner in `repository/mod.rs`. Retain the existing method receivers. Use `pub(super)` for a formerly private helper shared only by repository siblings. |
| Construction-only helpers mixed into the old root | Move `map_binding_value` and `string_values` to their sole production consumer, `construction/batch.rs`. Put the shared construction helper `canonical_custom_text_key` in `construction/batch.rs` with visibility for its construction siblings. Put `finish_fresh_fingerprints` beside its caller in `construction/repository_state.rs` and its regression beside that owner. Replace the test-only forwards of `prepare_discovered_file`, `prepare_discovered_window` and `SegmentJobGate` with imports from their existing construction owners. |
| Remaining `repository.test.rs` cases | Move repository lookup and count tests to `repository/tests.rs`. Place the metadata preparation test with construction discovery. Register each test once through its owning module. Retain the assertions and map every old test name to its new name. |

Remove exactly these five superseded root files on the reviewed layout: `repository.rs`, `repository.test.rs`, `repository_configuration.rs`, `repository_configuration.test.rs` and `configuration.rs`. Retain `pub mod repository;` in `lib.rs` and remove the obsolete root module declarations. Update all maintained callers to the paths above in the same compiling commits. Do not leave old-path re-exports, file shims, type aliases, duplicate types or cross-directory `#[path]` redirects. Private implementation modules containing inherent methods need no public module path. Leave unrelated roots and historical reports in place.

## 3. Preserve the execution and publication contracts

1. **Construction and sources.** `Index::start_build` followed by the existing task methods remains the public build path. `construction/repository_state.rs` continues to initialize and assemble the state. Lifecycle owns initial coordination and publication. Do not introduce a repository builder, second pass over rows, second registry, or alternate state assembly path. Keep the resolved reader on each collection and obtain its scalar verification capability from the resolved registration. Preserve relative paths, JSONL byte ranges, raw fallback and unavailable logical payloads.
2. **Pinning.** Preserve the actual root passed by `IndexView` to the executor or reference evaluator. A pin includes its repository instance, revision, active domains, segments and catalog. An older live view continues to produce its original membership and diagnostic identities after publication. The repository-native operations retain their own existing domains. Keep the active-domain arguments and distinct hit-stamping rules. Calling a repository helper from a pinned view must never substitute repository-base state or reacquire the latest view.
3. **Query and results.** Move existing dispatch bodies mechanically. Preserve one clock resolution per request, parameter-binding errors, collation and tie order, complete versus unknown counts, pagination, partial results, typed payloads, diagnostic remapping and byte ranges. Keep reference evaluation independent of candidate selection, indexes and stored sort keys. Keep eligible selector-free reuse and occurrence-aware evaluation for explicit selectors. Result moves must introduce no cloning of bitmaps, rows, payload buffers or cached result pages. Streaming keeps borrowed slices and the existing conditional copy only on paths that already remap rows.
4. **Lifecycle and caches.** Preserve instance and revision scoping of cache keys, generation stamps, worker counts, admission and cancellation. Keep expected-revision and quiescence checks inside the publication write lock. Maintain existing lock order, worker join behavior, retirement after unlock and sharing of unaffected segment storage. Imported type paths may change across these modules. Leave the algorithms with their current owners.
5. **Compatibility.** The intended public change is Rust import spelling for moved items. Preserve CLI arguments and output, desktop data contracts, query grammar and semantics, configuration identity, persisted schema/snapshot representations, budgets and profiling fields. Compile public positive imports as well as the existing negative privacy doctest; a negative test can pass merely because an import is broken.

## 4. Implementation order

1. Move the three result definitions to `query_results` and migrate consumers in one compiling commit. Search `crates/{core,cli,desktop}`, including benches, examples, integration tests, doctests and test support. Known consumers include CLI `execution.rs`, `output.rs`, `tui/` and desktop `backend/{export,query,record,record_json}.rs`. Preserve public return types and every match arm on `Payload`.
2. Convert `repository.rs` to the directory module. Move configuration, loader and identity hashing, remove both obsolete root modules, and migrate callers. Include CLI `loader.rs`, desktop `backend/open.rs` and the Musenalm example support. Check both configuration identity regressions and the loader suite immediately.
3. Extract state, statistics and the three operation impl files. Relocate the identified construction helpers and test-only imports in the same change as their callers. Use moved-code diffs to account for any altered body. No change to a lock scope, read count, borrow duration or data representation is required by this assignment.
4. Migrate or extend the public caller test below. Update maintained API documentation and snippets referring to the superseded paths. Keep historical benchmark evidence and completed task documents as records of their own revisions.
5. Inspect `git diff --find-renames B7..HEAD` and the final root inventory. Look for old qualified paths, grouped imports, module aliases and duplicate declarations. Confirm every moved public item resolves from exactly one canonical path; only `repository::Repository` intentionally retains its original public name among the moved repository definitions.

## 5. Public acceptance test

Extend `crates/core/tests/inline_sort.rs` using its existing public fixture and both real entry points. Use the external crate spelling throughout. Its five records and declared sort fields already support this acceptance case.

- Configure a collection with `repository::configuration::{CollectionDefinition, IdentityPolicy, RepositoryOptions}`, `schema::{Schema, FieldDefinition}`, `source::SourceFormat` and `source::registry::SourceRegistry`. Build with `lifecycle::Index::start_build`, wait for completion and obtain `index.view()`. Use the public `query::Query` entry point.
- Request `year = 3 SORT BY title ASC` over the existing fixture, with count enabled, offset zero and limit two. The full matching IDs are `c, d, b`: `c` and `d` have title `Alpha`, `b` has `Zulu`. Assert first page `[c, d]`, second page at offset two `[b]`, and `count == Some(3)` on both. Execute the assertions on both `IndexView::search` and `IndexView::search_reference`. Use explicitly typed `query_results::SearchResult` and borrowed `&query_results::SearchHit` in the caller.
- For locator mode assert the collection, relative file path, generation tied to the view, correct range and absent payload. On the same view request `ReturnMode::Logical` and inspect `Some(query_results::Payload::Logical(..))`, including the expected record field. For byte mode assert the bytes equal the fixture contents for that range. Use existing focused fallback and stale-source tests for the unavailable variant. Shut the index down before cleaning the fixture.
- Close one small test-evidence gap in the A5 carry-forward test `scalar_relation_wildcards_do_not_reuse_selector_free_sort_keys`. Its B7 version checks optimized one-row pages and compares full-page hit vectors across engines. Add direct one-row **reference** assertions for wildcard `[m1]` and selector-free `[m2]`, and assert `Some(2)` on the full and one-row reference results for both. Keep this as a test-only change. A failure is a behavior finding and must be reported separately from a source move.

## 6. Verification and performance

Run these commands on the combined final code revision. Record the actual command, exit status, passed/failed/ignored counts and tested SHA. Retain the feature inventory separately because feature-specific tests differ.

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked -p objekte-search-core --all-features
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps
cargo test --locked -p objekte-search-core --test inline_sort
cargo test --locked -p objekte-search-core --lib lifecycle::
env -u OBJEKTE_LARGE_COMPACT_RECORDS cargo test --release --locked -p objekte-search-core --lib large_source_free_compaction_cancels_and_retries_a_concurrent_commit -- --ignored --nocapture
```

Run and record positive execution for each selection below. If test paths move, use the recorded name mapping. A filtered run with zero executed tests leaves that gate pending.

| Contract | Required focused evidence |
| --- | --- |
| Configuration and repository | All loader tests, both configuration-identity tests including `live_configuration_hash_matches_the_pre_restore_fixture`, `fresh_fingerprints_require_every_shared_binding_to_agree`, `direct_id_lookup_matches_search_metadata_payload_and_stale_semantics`, `exact_count_uses_index_proofs_and_resolves_parameters_and_frozen_time`, and the external caller test. |
| A4 reader resolution | `custom_readers_under_builtin_names_never_use_builtin_scalar_verification`, `borrowed_source_verification_matches_reference_for_outputs_and_duplicates`, `root_scoped_fts_matches_reference_across_readers_and_outputs`, and `decoded_scalar_fts_combines_source_text_with_required_values`. |
| A5 execution | Run `relation_query::tests::` and `query::sort_binding::tests::`. Explicitly account for the scalar wildcard test, `incomplete_backlink_discovery_stops_before_target_sort_cache_work`, the `base_reference_` controls, and `segmented_reference_parity_with_retired_and_new_rows_and_parameter_clock`. Retain assertions for zero sort-target reads in the two repaired cases and the eight successful 2 MiB reads during incomplete discovery. |
| A6 lifecycle | Run `maintenance_admission_shares_only_the_real_worker`, `concurrent_quiesce_and_shutdown_wait_for_the_same_worker`, `pinned_view_stays_stable_when_a_new_root_is_published`, and the full lifecycle group. The explicit ignored release run above uses the default 262,144 records. Capture attempts, before/after membership and the one-test result. Preserve the stale/foreign publication and concurrent-mutation tests in the suite. |

Use `bench/README.md` and the maintained release examples for a matched build and query comparison on B7 and final code when the benchmark corpus is available. Reuse the same corpus, compiler/profile, worker counts, alphabet, cache configuration and cases for each side. Record the commands, repeat count, build time and peak RSS, query timings, ordered-result checks, candidates, parses and source reads. Use multiple repetitions for any timing conclusion. The earlier one-run Musenalm report at `f2020699` is historical context. It cannot substitute for a measured B7 baseline. For a pure move, list the matched operation counts and moved bodies; explain any benchmark limitation. If a hot-path body, data structure or lock scope changes, isolate the difference and measure the affected operation before requesting acceptance. Investigate increased I/O, allocation or repeated traversal even if a small fixture still passes.

## 7. Completion and handoff

Push each compiling commit to the integration branch under the repository workflow. Finish the task document in `docs/ai/done/objekte_core_repository_operations_and_state_specification.md` with its implementation report. Include B7, the tested code SHA, old/new symbol and test paths, five root removals, caller migration, actual validation, benchmark evidence and outstanding items. Identify the separately pushed report/delivery SHA in the handoff. Report any behavioral diff explicitly. The owner reviews and merges the delivery.

[Roadmap base](https://github.com/Simon-Martens/objekte/tree/1bafff6fa53be3215f0b48731550cce569f699c3/crates/core/src) · [Reviewed repository implementation](https://github.com/Simon-Martens/objekte/blob/04a057b74e37e4c13df1736edca9214a736b58f2/crates/core/src/repository.rs) · [Public caller fixture](https://github.com/Simon-Martens/objekte/blob/04a057b74e37e4c13df1736edca9214a736b58f2/crates/core/tests/inline_sort.rs) · [Prerequisite repair report](https://github.com/Simon-Martens/objekte/blob/04a057b74e37e4c13df1736edca9214a736b58f2/docs/ai/done/assignment_5_relation_sort_repair_addendum.md)

## Implementation report

### Completion metadata

- Task identifier: `core-repository-operations-and-state`
- Completion date: 2026-11-19
- B7: `6f3c3ee8aa5a49cf82d83905b5eb212c1e984b46`
- Integration branch: `integration/core-repository-operations-and-state`
- Integration worktree: `/home/simon/source/objekte-integration-core-repository-operations-and-state`
- Final validated source revision: `0e8708fcfd302b356d026c344dd41491ed2cec89`
- Repaired documentation/evidence candidate: `d235605417b037eca76ea62101074914ea77845d`
- Last source-behavior implementation commit: `3bf4c1f7c9167bfed012fe7011c553bc72a16dd9`
- Benchmark evidence commit: `fed6cdf8d187e7dea7771afcf4f9481b9f8e5cc0`

The implementation is integrated, validated, and pushed. The task document is completed by the separately pushed report commit identified in the handoff. The owner still owns the merge into `master`; this task deliberately did not modify `master`.

### Implemented ownership

The repository root is now a directory module. `Repository`, its single process-wide instance counter, worker construction, verified-cache configuration, and `empty_like` remain with the resident owner in `repository/mod.rs`. The module documentation links to the maintained public caller test.

| B7 symbol or source | Final owner |
| --- | --- |
| `Repository`, `next_repository_instance` | `repository/mod.rs`; public `Repository` path remains `repository::Repository` |
| `IdentityPolicy`, `CollectionDefinition`, `RepositoryOptions` | `repository/configuration/mod.rs` |
| `RepositoryConfiguration`, `ExplicitSchemaFile`, `load_repository_configuration`, `load_explicit_schema` | `repository/configuration/loader.rs` |
| `configuration_hash` | `repository/configuration/identity.rs` |
| `CollectionState`, `CollectionStats`, `GlobalInferenceSummary` | `repository/state.rs` |
| `StartupStats`, `BuildProfileStats`, `IncrementalSegmentProfile`, `BuildContributionStorageProfile`, profiling RSS helper | `repository/statistics.rs` |
| `SearchHit`, `Payload`, `SearchResult` | `query_results/mod.rs` |
| Repository lookup and hit-stamping operations | private `repository/records.rs` |
| Repository inspection, relation, inference, cache, segment, signature, and statistics accessors | private `repository/inspection.rs` |
| Optimized/reference search, count, explain, parameter/context, and streaming dispatch | private `repository/search.rs` |
| `map_binding_value`, `string_values`, `canonical_custom_text_key` | `construction/batch.rs` |
| `finish_fresh_fingerprints` | `construction/repository_state.rs` |

The five superseded direct root files were removed exactly as required:

- `crates/core/src/repository.rs`
- `crates/core/src/repository.test.rs`
- `crates/core/src/repository_configuration.rs`
- `crates/core/src/repository_configuration.test.rs`
- `crates/core/src/configuration.rs`

The direct `crates/core/src/*.rs` inventory fell from 48 files at B7 to 43. A definition-count audit found exactly one definition for every moved public type. A workspace search found no old `configuration`, `repository_configuration`, or query-record import spellings, no old-path re-exports, type aliases, duplicate definitions, or redirects into the removed files. Historical reports that mention the old filenames remain unchanged.

Maintained callers were migrated in the same compiling changes, including CLI configuration loading and output, desktop backend/controller code, core integration tests, examples, Musenalm support, and internal construction/lifecycle/query users.

### Test ownership mapping

The default inventory remains 621 tests and the all-feature inventory remains 622 tests. The following ten intended path changes are the complete inventory-name delta in both modes:

| B7 test path | Final test path |
| --- | --- |
| `repository::tests::parallel_metadata_preparation_keeps_order_revisions_and_overlapping_memberships` | `construction::discovery::tests::parallel_metadata_preparation_keeps_order_revisions_and_overlapping_memberships` |
| `repository::tests::fresh_fingerprints_require_every_shared_binding_to_agree` | `construction::repository_state::tests::fresh_fingerprints_require_every_shared_binding_to_agree` |
| `repository_configuration::tests::repository_layout_loads_sorted_definitions_and_protected_schema_targets` | `repository::configuration::loader::tests::repository_layout_loads_sorted_definitions_and_protected_schema_targets` |
| `repository_configuration::tests::explicit_schema_reads_only_selected_file_and_reports_canonical_protection` | `repository::configuration::loader::tests::explicit_schema_reads_only_selected_file_and_reports_canonical_protection` |
| `repository_configuration::tests::repository_configuration_rejects_duplicate_collection_names` | `repository::configuration::loader::tests::repository_configuration_rejects_duplicate_collection_names` |
| `repository_configuration::tests::repository_configuration_never_searches_parent_directories` | `repository::configuration::loader::tests::repository_configuration_never_searches_parent_directories` |
| `repository_configuration::tests::source_preparation_preserves_live_identity_and_generated_exclusions` | `repository::configuration::loader::tests::source_preparation_preserves_live_identity_and_generated_exclusions` |
| `repository_configuration::tests::source_selection_rejects_generated_namespaces_and_aliases` | `repository::configuration::loader::tests::source_selection_rejects_generated_namespaces_and_aliases` |
| `configuration::tests::live_configuration_hash_matches_the_pre_restore_fixture` | `repository::configuration::identity::tests::live_configuration_hash_matches_the_pre_restore_fixture` |
| `configuration::tests::configuration_identity_tracks_content_inputs_and_ignores_runtime_budgets` | `repository::configuration::identity::tests::configuration_identity_tracks_content_inputs_and_ignores_runtime_budgets` |

The lookup and count tests retain their names under `repository::tests`:

- `direct_id_lookup_matches_search_metadata_payload_and_stale_semantics`
- `exact_count_uses_index_proofs_and_resolves_parameters_and_frozen_time`

Each moved test is registered once through its owning module. No moved test lost assertions.

### Public caller and execution contracts

`crates/core/tests/inline_sort.rs` now contains
`defining_public_modules_configure_build_query_pin_and_inspect_results`. It builds and queries through the defining public modules, uses the external crate spelling, types `SearchResult` explicitly, and borrows `&SearchHit`. It verifies the `year = 3 SORT BY title ASC` results `[c, d]` and `[b]`, exact count `Some(3)`, locator mode, logical payload, byte payload, and both optimized and reference entry points.

The A5 wildcard regression now directly verifies one-row reference pages and exact counts for wildcard `[m1]` and selector-free `[m2]`, in addition to the existing optimized and full-page checks.

The query-record move, repository split, and helper relocations are source/ownership changes. The implementation did not introduce a second construction path, alternate repository builder, data-representation change, or alternate lock order. Existing publication, pinning, query, result, streaming, cache, and lifecycle algorithms remain with their B7 owners.

### Validation

All final gates ran sequentially against `0e8708fcfd302b356d026c344dd41491ed2cec89`.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | passed |
| `cargo check --locked --workspace --all-targets` | passed |
| `cargo test --locked --workspace` | passed |
| `cargo test --locked -p objekte-search-core --all-features` | passed |
| `RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps` | passed |
| `cargo test --locked -p objekte-search-core --test inline_sort` | 5 passed |
| `cargo test --locked -p objekte-search-core --lib lifecycle::` | 89 passed, 6 ignored |
| Default-size ignored release compaction gate | 1 passed; details below |

The workspace test run included:

- Core unit tests: 605 passed, 16 ignored.
- Core integration tests: 29 passed across all integration targets.
- Core doctest: 1 passed, including the `Repository` compile-fail privacy example.
- CLI: 81 library tests, 33 process tests, and 1 removed-flags test passed; 1 process test ignored.
- Desktop: 161 passed, 1 ignored.

The all-feature run included 606 passed and 16 ignored core unit tests, the same 29 passing core integration tests, and the passing core doctest.

Focused contract evidence also passed:

- Repository configuration and identity: 8 passed.
- Moved repository lookup/count tests: 2 passed.
- Moved fresh-fingerprint regression: 1 passed.
- A4 reader-resolution controls: all 4 passed individually.
- `relation_query::tests::`: 33 passed.
- `query::sort_binding::tests::`: 12 passed.
- A6 `maintenance_admission_shares_only_the_real_worker`: 1 passed.
- A6 `concurrent_quiesce_and_shutdown_wait_for_the_same_worker`: 1 passed.
- A6 `pinned_view_stays_stable_when_a_new_root_is_published`: 1 passed.

The A5 focused groups report positive execution for
`scalar_relation_wildcards_do_not_reuse_selector_free_sort_keys`,
`incomplete_backlink_discovery_stops_before_target_sort_cache_work`,
the three `base_reference_` controls:

- `base_reference_sort_ignores_perturbed_stored_and_relation_contributions`
- `base_reference_resolves_omitted_id_map_targets_and_materializes_their_bytes`
- `base_reference_multi_target_backlinks_scan_each_relation_once`

The separate fourth control is
`segmented_reference_parity_with_retired_and_new_rows_and_parameter_clock`.
Their retained assertions cover zero sort-target reads in the two repaired cases and eight successful 2 MiB reads during incomplete discovery.

The required release compaction command was:

```sh
env -u OBJEKTE_LARGE_COMPACT_RECORDS cargo test --release --locked \
  -p objekte-search-core --lib \
  large_source_free_compaction_cancels_and_retries_a_concurrent_commit \
  -- --ignored --nocapture
```

It used the default 262,144 records and passed:

```text
large compaction build records=262144 elapsed=247.923419ms
large compaction records=262144 elapsed=251.902327ms attempts=2 before=262146 after=262145 retired=1
test result: ok. 1 passed; 0 failed; 0 ignored
```

One earlier core-unit run overlapped another full test process and hit the bounded ten-second wait in
`lifecycle::initial::tests::private_final_drain_consumes_worker_error_before_returning`.
The test passed when rerun alone (`1 passed`, 1.72 seconds) and passed again in the final sequential workspace and lifecycle runs. This is recorded as contention from the concurrent validation attempt; no implementation change was made for it.

Default and all-feature `-- --list` inventories were compared with B7. Both remain 621 and 622 tests respectively, with only the ten mapped names above changing. The three crate test-target inventories remain unchanged; the new public acceptance case was added to the existing `inline_sort` target.

The repair candidate `d235605417b037eca76ea62101074914ea77845d` reran
the requested documentation/evidence gates: formatting, all-target workspace
check, all 5 `inline_sort` tests, all 3 `base_reference_` tests, the separate
`segmented_reference_parity_with_retired_and_new_rows_and_parameter_clock`
test, the core doctest, and workspace rustdoc with broken links denied. The
corrected `docs/inference.md` snippet was also compiled in an isolated Cargo
project against that worktree. No production source behavior changed after
`0e8708fcfd`, so the prior full workspace, all-feature, and release compaction
results remain the applicable behavioral gates.

### Benchmark

The original `repository-operations-{b7,final}` query directories are superseded. Those invocations used different physical corpus copies and therefore different timestamps. Their manifests have different corpus fingerprints and the compatibility check reports `corpus_fingerprint differs from baseline`; 139 non-empty ordered-result digests differed. The artifacts cannot establish whether timestamps or another omitted input caused all 139 differences, so the previous generation-only explanation is withdrawn.

The replacement evidence uses one canonical root for both sides:
`/home/simon/source/objekte/examples/musenalm`. Its pre- and post-run inventory metadata SHA-256 were both `ce54312379a363b9a76203ba072967d4ec80aa3422eb76c31f83c942b1589901`, covering paths, nanosecond modification times and sizes. The corpus contains 148,486 source records across seven collections. The query manifests record common corpus fingerprint `9f93a4ba7fb9249c9c4eb04e03ed911435a6b0493f15b087a5447add32456aa9` and common schema fingerprint `bf36ea2e88d1b455e28b0f5f36c71ebd03847b2a986cd3f1f2ea3302fd1eea08`.

B7 and the last source implementation were built from their respective worktrees with their tracked source clean, into distinct target directories with:

```text
OBJEKTE_BUILD_REVISION=6f3c3ee8aa5a49cf82d83905b5eb212c1e984b46
OBJEKTE_BUILD_REVISION=43005ca3e959d678614998ead6a76a444e7a62cc
cargo build --release --locked -p objekte-search-core \
  --example bench_musenalm --example bench_build_musenalm
```

Both used `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, the release profile, the default build flags and the performance CPU governor. The benchmark executables were:

| Side | Query executable SHA-256 | Build executable SHA-256 |
| --- | --- | --- |
| B7 | `edd3d97f95683797b9e6f9d6202a7cdc0ef55d88ca455b5b0a68d295f2b6e80a` | `b6f82a81302b8adfb2d4f1df11f1d13d6b48351376b4d36d9ee267318ec20841` |
| Candidate | `16a15606e0ad51c71d607c9c869f8c5b7fa97e15204098468d7719eeec7bbfd6` | `5c85513a3b7ec8d977959f5c84a7370a88075297aa9d33905969d86f7809beab` |

Each side ran three query repetitions with:

```sh
bench_musenalm --root /home/simon/source/objekte/examples/musenalm \
  --profiles ascii-off --repetitions 3 --query-workers 4 \
  --verified-cache-bytes 0 --result-dir <side>/queries
```

The compiled workload fingerprints differ because `bench_musenalm` hashes the compiled-in workload path, but the actual workload files are byte-identical (`56f2ba99e3a889b95975a6b460c4ef894732ad769f99e6f69086a3f0d3ac91c0`), and the resolved `queries.json` and `schema-manifest.json` files are byte-identical. The existing compatibility function returns `[]`.

All 450 sample keys matched. All 150 ordered-result digests matched, covering 139 non-empty pages and 11 empty pages. Status, count, returned page size, every recorded work-counter field and ordered-result identity had zero differences. The compact result is `bench/results/musenalm/repository-operations-common-root/comparison.json`.

Direct build samples used the same root with `--build-workers auto`, three ASCII and three Unicode samples per side. Both alphabets produced 148,486 records and parses with zero failed parses. Index signatures now agree across revisions: ASCII `4a38808629a1544a354d715e42c85ac426b1cfe93376e862253b3d5cc03f349f` and Unicode `5c235a33b6cb1a18c1a1f4d2b1d60fa1eae912ce2b4ff868857df95028d83868`.

Build medians:

| Side | Alphabet | Median | Range | Peak RSS |
| --- | --- | ---: | ---: | ---: |
| B7 `6f3c3ee8aa` | ASCII | 0.643 s | 0.635–0.647 s | 624.1 MiB |
| Candidate `43005ca3e9` | ASCII | 0.635 s | 0.620–0.635 s | 617.2 MiB |
| B7 `6f3c3ee8aa` | Unicode | 0.736 s | 0.701–0.737 s | 629.7 MiB |
| Candidate `43005ca3e9` | Unicode | 0.682 s | 0.682–0.699 s | 627.5 MiB |

Both query sides ran 150 cases with three samples per case and zero failures. Aggregate per-case medians:

| Side | Sum | Mean | Median | P95 | Worst |
| --- | ---: | ---: | ---: | ---: | ---: |
| B7 | 1,395.069 ms | 9.300 ms | 0.332 ms | 25.343 ms | 274.824 ms |
| Candidate | 1,387.970 ms | 9.253 ms | 0.287 ms | 25.577 ms | 270.577 ms |

The new measurements show a 1.3% lower ASCII build median, a 7.4% lower Unicode build median and slightly lower peak RSS. Query aggregate time and worst-case time were slightly lower while P95 was 0.9% higher; these differences are not material and no performance threshold applies to this source-only refactor. The raw query runs and direct build samples are under `bench/results/musenalm/repository-operations-common-root/{b7,candidate}/`; build medians and signature comparisons are in `build-comparison.json`.

### Commits and completion status

Implementation and evidence commits on the integration branch:

- `3741efd0b6adc8e327ecee17e1fb6e82ac22eae5` — move query result records to `query_results`.
- `3bf4c1f7c9167bfed012fe7011c553bc72a16dd9` — establish canonical repository module ownership.
- `0e8708fcfd302b356d026c344dd41491ed2cec89` — link the public caller test from repository module documentation.
- `fed6cdf8d187e7dea7771afcf4f9481b9f8e5cc0` — add the matched B7/final Musenalm evidence.
- `d235605417b037eca76ea62101074914ea77845d` — replace the invalid comparison with common-root evidence, correct maintained documentation, and add the ordered-query/build comparison tools.

There are no approved deviations, behavioral diffs, or unresolved implementation issues. The precise cause of the superseded different-root digest differences remains unestablished, but the common-root replacement establishes complete ordered and work-counter parity. No subordinate worktrees were created. Task memory is removed after this report is pushed; the integration branch and worktree remain for owner review and merge.
