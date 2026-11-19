# Assignment 5 — Schema ownership, query binding and independent evaluation

**Task** `core-schema-binding`  
**Integration branch** `integration/core-schema-binding`  
**Sibling worktree** `/home/simon/source/objekte-integration-core-schema-binding`  
**Implementation base** The exact pushed Assignment 4 delivery commit (**A4**). The symbol inventory refers to master `bba4d0a8c3ce1e6a39ce6b04dd0469aedb83b40d`, which includes the interim cleanup. Use Assignment 4’s new source paths.

## Overnight handoff

Finish Assignment 4 on `integration/core-source-refactoring`. Record **B** (its base), run its required validation, push its completion report, and record **A4**. Leave that branch at A4. The overnight instruction authorizes starting 5 as soon as 4 passes, without waiting for a merge or morning approval.

Create a separate sibling worktree from A4. For a fresh branch, set `A4` to the full verified SHA and run this from the original checkout:

```sh
git worktree add -b integration/core-schema-binding ../objekte-integration-core-schema-binding "$A4"
```

If the branch already exists, inspect its worktree, ancestry and pending changes, then resume safely. Follow `docs/ai/README.md` for task records and pushing after every commit. Keep implementation and validation in the assigned worktrees. The owner makes a separate merge decision for each completed assignment in the morning.

## Design decision

Schema declares fields and extension operations. Binding resolves FILTER and FTS syntax into typed requests for one collection. The independent evaluator applies the same logical expressions to visible records on a pinned revision. These responsibilities currently share `reference.rs`. Make each separately discoverable. A change to posting storage should require no change to exact evaluation, and registering an operation should require no import from the reference evaluator. [Schema and operations](https://github.com/Simon-Martens/objekte/blob/bba4d0a8c3ce1e6a39ce6b04dd0469aedb83b40d/crates/core/src/reference.rs#L29) · [FILTER binding](https://github.com/Simon-Martens/objekte/blob/bba4d0a8c3ce1e6a39ce6b04dd0469aedb83b40d/crates/core/src/reference.rs#L1191) · [FTS binding](https://github.com/Simon-Martens/objekte/blob/bba4d0a8c3ce1e6a39ce6b04dd0469aedb83b40d/crates/core/src/reference.rs#L2802)

## File and symbol ownership

All paths below are relative to `crates/core/src/`. Define each moved symbol in its canonical module. Migrate core, CLI, desktop, examples, integration tests and active documentation in the same change. Use module-local tests and ordinary child declarations. Delete former root files and aliases.

| Destination | Required contents |
| --- | --- |
| `schema/mod.rs` | `Schema`, `FieldDefinition`, field/default/raw registration and lookup, auto-index exclusions, configuration fingerprint and composite-sort declarations. Preserve schema state and registration order semantics. |
| `schema/operation.rs` | `Transform`, `CustomBinding`, `RegisteredBinding`, `BindingOperation`, `ExactInput`, `PreparedFilterMatcher` and `PreparedTextMatcher`. This is the public extension contract. Preserve object safety, versions, default methods, and selected-value versus original-record-bytes input. |
| `schema/validation.rs` | `ValidationNode` and its compiled trees, declared-type and child validation, indexed conversion, conversion flags and their helpers. Keep `coerce_record`, `coerce_projected_record` and `convert_indexed_record` inherent on `Schema`. Preserve required/optional ancestors, nested array nullability, root shapes and projection. |
| `schema/json.rs` | Move `schema_adapter.rs`, `CompiledSchema`, `load_schema_file`, `compile_schema` and their tests. JSON is the configuration format here. Compiled field declarations still apply to all source readers. |
| `query/binding/filter.rs` | `BoundExpr`, `BoundTerm`, `BoundFieldOperand`, `Derived`, `bind_filter`, parameter-aware binding, `bind_field_probe`, literal preparation, explicit array scopes and `same_bound_term`. Preserve type unification, relation operands, spans, matcher identity and source kind. |
| `query/binding/fts.rs` | `BoundFts`, `BoundFtsField`, `bind_fts`, regex/raw bindings, regular and fallback literals, prepared finders and the existing gram requests. Give exact matching borrowed read access to the existing finders. |
| `query/binding/matcher.rs` | `MappedFilterMatcher`, `MappedTextMatcher`, `RegexTextMatcher` and retained-byte accounting for their prepared state. They implement the public traits in `schema::operation`. |
| `query/binding/relation.rs` | `HopDirection`, `BoundRelationHop`, `BoundRelationPath`, `bind_filter_with_relations`, `bind_fts_with_relations`, `bind_related_term`, `resolve_path` and binding-only helpers. Continue to resolve collection declarations, backlinks, array selectors and target reader text derivation. Adjacency traversal, caches, semijoins and execution modes remain with relation execution. |
| `query/semantics/state.rs`, `record.rs` and `comparison.rs` | Move `StatePredicate` and its tests into `state.rs`. Place `RecordContext`, metadata extraction and borrowed path selection in `record.rs`. Place scalar comparison, field-to-field scope pairing, `PreparedContains` and wildcard matching in `comparison.rs`. These are shared record-level operations. |
| `query/semantics/folding.rs` | Per-record fold guards, worker scratch, shared fold cache and test instrumentation. Preserve reuse within a record, reset at record boundaries, bounded retention and the existing guard lifetime. |
| `reference/filter.rs` and `reference/fts.rs` | FILTER owns `eval_term`, `eval_filter`, unavailable-source truth and membership. FTS owns full/field evaluation, raw fallback, membership, evidence and the borrowed matching routines. Move reference tests to their behavior owners. `reference/mod.rs` declares these modules. |

Use `schema::Schema`, `schema::FieldDefinition`, `schema::operation::BindingOperation`, `query::binding::filter::bind_filter` and `query::binding::fts::bind_fts` as canonical public imports. Keep `query/prepared.rs` responsible for request preparation, term IDs and collection bindings. Keep logical relation declarations in `crate::relation`, including schema relation registration next to its validation. Remove the root pairs for `reference`, `schema_adapter` and `field_state`, including their three companion tests. On the specified post-4 layout this takes the direct Rust file count from **67 to 61**. Check the six actual paths as well as the count.

## Resolve three existing dependencies

1. **Unicode folding.** The evaluator currently imports `fold` and `fold_into` from `index::bigram`. Put those functions and `folded_chars` in `model/text.rs`, declared under the existing `model.rs`. Both exact matching and gram indexing import the same implementation. Preserve full non-Turkic folding, ASCII handling and caller-owned buffer reuse. Keep physical gram encoding and construction loops in the index. Existing private `BigramRequest` fields may travel on a bound request as opaque hints computed during binding. Exact evaluation ignores those hints. Preserve per-field, fallback and regex cases. Do not recompute hints for every segment or record. [Current fold and gram request](https://github.com/Simon-Martens/objekte/blob/bba4d0a8c3ce1e6a39ce6b04dd0469aedb83b40d/crates/core/src/index/bigram/mod.rs#L23)
2. **Composite sorts.** `Schema::register_composite_sort` currently calls `index::sorting::simple_composite_part`. Move the logical descriptor `CompositeSortPart` and its normalization function to the existing `execution_options` module. Schema registration and physical sorting use that same definition. Preserve path parsing, descending flags, binding/reducer rejection and duplicate detection. The resident order and slot array stay with physical sorting. [Current normalization](https://github.com/Simon-Martens/objekte/blob/bba4d0a8c3ce1e6a39ce6b04dd0469aedb83b40d/crates/core/src/index/sorting/mod.rs#L161)
3. **Independent truth evaluation.** Preserve `IndexView::search_reference` and its parameterized form. The reference scan can use shared exact scalar matching, but must visit its own visible records. Keep production reference code free of plan decisions, candidate bitmaps, provider calls and proof caches. The present `eval_term` also calls a test-only alternate matcher. Move the existing guarded test registry, guard and counters into `query/semantics/test_matcher.rs` if needed to remove its reverse dependency on `query_predicate`. Keep the same lookup, synchronization, short-circuit and invocation counts in its two callers. [Exact evaluator](https://github.com/Simon-Martens/objekte/blob/bba4d0a8c3ce1e6a39ce6b04dd0469aedb83b40d/crates/core/src/reference.rs#L2411) · [Prepared verification](https://github.com/Simon-Martens/objekte/blob/bba4d0a8c3ce1e6a39ce6b04dd0469aedb83b40d/crates/core/src/query_predicate/mod.rs)

## Behavior and performance requirements

- Retain plain and escaped wildcard matching, case folding, regex modes, raw fallback and error propagation. Preserve the fields included in evidence for true Boolean branches. Membership-only execution may short-circuit. Unavailable record data remains unknown under negation; a metadata term can still decide a conjunction or disjunction.
- Preserve extension acceptance and opt-ins for posting, bigram, coverage, prefix, order and regex operations. Keep the difference between a selected logical value and an operation consuming original source bytes. Keep pointer identity for shared matcher operations, including the existing `Arc::ptr_eq` checks. Preserve schema and registration fingerprints.
- The provider chooses how to produce membership. The query planner chooses among the offers for FTS or FILTER terms by cost and applies them to the current domain. Keep the existing borrow/move behavior for results and proofs. No new bitmap clone, copied evidence set, second binding walk, or source read to pass data across modules. Preserve Assignment 4’s reader dispatch, including custom readers installed under built-in spellings.

## Implementation and validation

1. Record A4, root-file inventory, baseline tests and test names. First move schema, validation, extension contracts and the JSON schema configuration compiler. Keep each commit compiling and migrate downstream callers with the corresponding move.
2. Extract typed FILTER, FTS and relation binding. Move the two shared normalization operations and the prepared matcher implementations. Verify that the same prepared matchers and precomputed hints reach the same callers. Keep this change separate from splitting evaluation.
3. Extract exact record semantics and full-scan evaluation. Rehome private tests with their owner. Finish caller migration and remove obsolete files and imports. Inspect the diff for ownership changes, particularly `clone`, `to_owned`, `collect` and fixed-size scratch resets introduced during the moves.
4. On one pinned view with identical parameters and frozen clock, compare optimized and reference results for an indexed term combined with a residual term. Compare IDs, deterministic order, total count and requested evidence. Retain explicit expected truth-table results so a common regression in a shared scalar routine is detectable.

| Concern | Existing evidence to retain and run |
| --- | --- |
| Schema and extensions | `declared_types_required_fields_and_array_nullability_are_enforced`, `incremental_indexed_value_paths_match_complete_rebuild_in_any_registration_order`, `optional_ancestor_of_required_temporal_child_stays_optional_during_query_conversion`. Compile maintained extensions against `schema::operation`. |
| FILTER and unavailable data | `membership_filter_and_unavailable_truth_match_evidence_evaluation`, `missing_null_empty_truth_table`, `evidence_excludes_false_boolean_branches`, `boolean_state_binding_rejects_invalid_forms_and_preserves_named_conversion`. Include the `array_positions`, `parameters` and `temporal_periods` integration targets. |
| FTS and allocation behavior | `fts_membership_stops_after_first_match_but_evidence_checks_every_segment`, `bound_contains_reuses_prepared_pattern_across_scalar_and_array_values`, `shared_fold_reuses_one_logical_occurrence_and_resets_at_record_boundary`, and Assignment 4’s custom-reader regression. |
| Planner and relations | `sorted_scalar_and_gram_backing_match_reference_membership`, `selective_text_reduces_the_input_to_the_structured_lookup`, and the `non_json_relations`, `relation_occurrence_oracle` and `temporal_regex_integration` targets. |

Run `cargo fmt --all -- --check`, `cargo check --locked --workspace --all-targets`, `cargo test --locked --workspace`, `cargo test --locked -p objekte-search-core --all-features` and `RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc --locked --workspace --no-deps`. Compare the captured test inventory by test name, allowing module-prefix changes. Keep feature-gated and ignored tests registered. Use existing visit, allocation and source-read counters. If a hot loop or allocation strategy changes, measure that operation in release mode on identical inputs. A full Musenalm suite is unnecessary for mechanical moves with unchanged work.

## Morning review

Each assignment gets its own implementation report, validation evidence and merge recommendation. Record tested code SHAs, commands, results, skipped tests and any reproducible baseline failure. After pushing reports, verify both remote heads and give the owner **B**, **A4** and **A5** as full SHAs. Supply GitHub comparison links for **B…A4** (sources) and **A4…A5** (schema and binding). Verify A4 is an ancestor of A5 and the first branch still points at the frozen A4. If anything is incomplete, identify the last passing commit, failing command and exact remaining work per assignment. Once the Assignment 5 checks pass and its report is pushed, freeze the branch at A5. Create `integration/core-lifecycle-coordination` in its own sibling worktree from that exact SHA and continue with `objekte_core_lifecycle_coordination_specification.md`. If 5 remains blocked, report the blocker and leave 6 pending. The owner reviews 4, 5 and 6 in order.

## Implementation report

- **Task:** `core-schema-binding`
- **Completion date:** 2026-11-19
- **Base A4:** `5b3804be019ddb55447c7ad7810786e801a553a5`
- **Integration branch:** `integration/core-schema-binding`
- **Integration worktree:** `/home/simon/source/objekte-integration-core-schema-binding`
- **Tested code revision:** `72f487df62`
- **Implementation commits:** `0937e83afa`, `26970708f1`, `c90c5d317b`, `4df360e1ce`, `72f487df62`
- **Status:** complete and ready for review; no merge into `master` was performed

### Summary

Schema declarations and extension contracts now live in `schema`; FILTER, FTS,
matcher, and relation binding live under `query::binding`; shared record
semantics live under `query::semantics`; and `reference::{filter,fts}` contains
only independent full-scan evaluation. The former `reference.rs`,
`reference.test.rs`, `schema_adapter.rs`, `schema_adapter.test.rs`,
`field_state.rs`, and `field_state.test.rs` root files were removed.

The planner still receives the same bound terms, provider offers, prepared
matcher identities, gram hints, relation paths, and cost inputs. Physical index
storage remains behind `query_access`; no posting tree or segment representation
was added to the binder or independent evaluator. Borrowed matcher and result
flows remain intact, including `Arc::ptr_eq` checks and worker scratch reuse.
Custom Assignment 4 reader dispatch remains intact.

The test-only alternate exact-matcher registry moved beside query semantics. It
is scoped to the specific bound term under test instead of globally disabling
the direct-source plan, preventing unrelated parallel tests from changing one
another’s execution path.

### Symbol and path map

| Canonical path | Contents |
| --- | --- |
| `schema/mod.rs` | `Schema`, `FieldDefinition`, field/default/raw registration, lookup, exclusions, fingerprints and composite-sort declarations |
| `schema/operation.rs` | `Transform`, `CustomBinding`, extension traits and selected-value/original-byte input contract |
| `schema/validation.rs` | Validation trees, declared-type/container conversion and all `Schema` coercion/conversion methods |
| `schema/json/mod.rs` | JSON schema compiler, `CompiledSchema`, file loading and schema configuration tests |
| `query/binding/filter.rs` | FILTER bound terms, parameter binding, field probes and prepared contains behavior |
| `query/binding/fts.rs` | FTS bound request/field structures, literal/regex binding and gram hints |
| `query/binding/matcher.rs` | Mapped FILTER/text and regex prepared matchers |
| `query/binding/relation.rs` | Relation direction/path types, FILTER/FTS relation binding and path resolution |
| `query/semantics/state.rs` | State predicates |
| `query/semantics/record.rs` | `RecordContext`, metadata extraction and borrowed path selection |
| `query/semantics/comparison.rs` | Scalar comparison, field-to-field pairing and wildcard matching |
| `query/semantics/folding.rs` | Per-record fold guards, worker scratch, shared fold cache and instrumentation |
| `model/text.rs` | Shared full non-Turkic fold and caller-owned buffer reuse |
| `reference/filter.rs`, `reference/fts.rs` | Independent FILTER and FTS evaluation |

`CompositeSortPart` and normalization now live in `execution_options`.
`query::binding::relation` is public, while evaluator-only helpers remain
crate-visible.

### Removed root paths

- `crates/core/src/reference.rs`
- `crates/core/src/reference.test.rs`
- `crates/core/src/schema_adapter.rs`
- `crates/core/src/schema_adapter.test.rs`
- `crates/core/src/field_state.rs`
- `crates/core/src/field_state.test.rs`

The direct core Rust-file count is 61 at the tested revision, down from 67 at A4.

### Validation

The following commands passed at `72f487df62`:

```text
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked -p objekte-search-core --all-features
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps
```

Default core results: 583 passed, 16 ignored. All-feature core results: 584
passed, 16 ignored. Workspace CLI, desktop, process integration and doc tests also
passed. Comparing the A4 and A5 test inventories after allowing module-prefix
changes reports no added, removed or renamed test functions; all feature-gated
and ignored registrations remain present.

Focused checks passed for:

```text
query_execution::runner::tests::gate_a_public_search_selects_either_provider_in_either_order
query_execution::runner::tests::selective_candidate_and_broad_exact_or_share_the_restricted_domain
query_access::independent::tests::sorted_scalar_and_gram_backing_match_reference_membership
query_access::independent::tests::selective_text_reduces_the_input_to_the_structured_lookup
relation_semijoin::tests::seeded_array_semijoins_match_an_index_free_expected_enumeration
source::registry::tests::custom_readers_under_builtin_names_never_use_builtin_scalar_verification
```

The complete `reference::tests` suite also passed (23 passed, one ignored
release-only benchmark). Existing allocation, source-visitation, fold-cache and
prepared-matcher tests confirm borrowed reuse and retained-byte behavior did not
change. This was a mechanical ownership refactor with no parser loop, data
layout, or I/O-schedule change, so no full Musenalm benchmark was required.

### Review notes

No unresolved functional issue or blocked check remains. The A5 report commit is
documentation-only after tested code revision `72f487df62`, and its source tree
matches that tested revision. The branch and worktree remain for the owner’s merge
decision.

### Repair amendment

Repair branch `repair/core-schema-binding` tested code revision
`9dfe5784f0d72d52d12939b919aae3d1459a417a`. It resolves reference clocks and
bindings once at the public entry, restores a source-backed segmented relation
oracle independent of planner/provider/posting truth, resolves related FTS,
FILTER and sort keys from the same source path, and moves prepared contains
ownership to query comparison semantics.

Detailed independent-oracle, matcher, test-split and validation evidence is
recorded in `docs/ai/done/assignment_5_reference_evaluation_repairs.md`. The R5
repair retains both R4 and frozen A5 ancestry and keeps the six original A5 root
removals.
