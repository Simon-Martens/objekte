# Objekte core completion · Assignment 2 of 3

## Source preparation, JSON conversion and record identity

**Outcome.** A contributor can follow a record from its resolved reader through construction, query verification and payload delivery. The declared format determines eligibility for relative-path identities. The installed reader determines which verification operations are available. These decisions differ observably for `Registered("json")`.

| Delivery item | Requirement |
|---|---|
| Roadmap coverage | Stage 4a and Stage 4b of the core refactoring completion plan |
| Series | `integration/core-refactoring-completion` in `/home/simon/source/objekte-integration-core-refactoring-completion` |
| Entry revision **A1** | The reviewed and pushed Assignment 1 delivery commit. Record its full SHA and the series baseline **B** from its report. The reviewed roadmap base is `0f1d5e5ffa566a911f926ffe564e49c0c69ded77`. |
| Delivery revision **A2** | Two compiling implementation commits, then a completion-report commit on the same branch. Push each commit. The owner reviews A2 before Assignment 3 begins. |

### Establish the starting point

Read `AGENTS.md`, `docs/ai/README.md`, `docs/development/code-guidelines.md` and the Assignment 1 report. Reuse the series worktree. The original checkout stays on `master`. Verify that the integration head contains the accepted module moves and that the local and published history agree. If this assignment has been partially implemented, identify the last verified step and preserve that work.

At A1, list the default and all-feature core unit tests and their ignored subsets. Reuse existing inventories for the same SHA. Record the six functions in `record_preparation.test.rs` marked `#[test]`. Locate all users of the retiring module, including qualified paths inside function bodies. Run the focused controls below against A1 before changing their definitions. Use the same fixture files across revisions whenever an assertion compares path, revision, byte offset or generated identity. Paths below are relative to `crates/core/src/` unless stated otherwise.

### Commit 1 · Move each preparation operation to its caller’s subsystem

Commit subject `Assign record preparation to construction, source verification and payload delivery`.

| Current item in `record_preparation.rs` | Defining file | Contract to preserve |
|---|---|---|
| `finish_build_record` | `construction/file_preparation.rs` | Apply `schema.coerce_record` before recursive finite-number validation. Both batch construction and pre-index scanning call this function. Existing callers own text finalization, diagnostics and fallback. |
| `prepare_query_record`, `prepare_query_record_with_execution`, `estimated_logical_bytes` | New `source/preparation.rs` | Keep the two entry points, signatures, projection, trusted-reader dispatch, validation, resource accounting and error classification. The logical-size estimator is private. |
| `prepare_decoded_record_with_borrowed_fts` | Existing `source/verification.rs` | Keep the single call to the resolved verification operation that produces the projected value and the FTS outcome from the same bytes. Preserve evidence, schema conversion and cancellation checks around that call. |
| `materialize` | Existing `query_results/payload.rs` | Consume the supplied `Vec<u8>` for bytes, move the prepared `Value` for logical output, and return `None` for a locator. Preserve the missing-logical-value error. |

1. Declare `pub(crate) mod preparation;` in `source/mod.rs`. Keep the other modules at their existing visibility. Use the Assignment 1 owners `query::options::ReturnMode` and `query_execution::streaming::{ExecutionContext, ExecutionErrorKind}`. The public result types stay in `query_results`.
2. Update construction, pre-index search, the query runner, repository posting-value inspection, relation evaluation, semijoins, relation target caching, sort binding, ranking, payload delivery and tests. Assignment 3 will move some of these *callers* later. Use their actual A1 paths in this commit. Delete the old module declaration and file once every caller builds from the new owners.
3. Preserve both extraction branches. For a validated logical record whose reader opts in, call `parse_for_query_trusted` and `convert_indexed_record`. Otherwise use the validating reader and `coerce_projected_record`. Preserve the projection's top-level selection and `collect_text` flag. A full logical result must include fields outside the filter projection. Raw fallback has no logical query value.
4. Preserve execution-aware ordering. The extension estimate may reject a record before parsing. A reader failure is classified as `Extension`, an excessive resulting value as `Resource`. Keep cancellation checkpoints and existing byte accounting. The path without an execution context retains its current cost. The assignment requires no new value walk for built-in readers.
5. Keep I/O at the caller that groups or bounds the read. The caller handling a raw fallback winner owns `Payload::LogicalUnavailable { diagnostic_ref }`. Keep the distinction between that public result and the preparation helper's missing-value error. For locator and bytes requests, preserve any reads the query itself needs for verification and add none for payload decoding.

**Test placement.** Move the five query-preparation tests below to `source/preparation/tests.rs`, keeping `built_and_trusted_logical` alongside them. Move `build_rejects_non_finite_numbers_before_indexing_nested_values` to `construction/file_preparation/tests.rs` and declare the suite there. Keep each original test name and assertion. The build/query agreement helper should import and invoke both real preparation functions and the payload function. Remove the obsolete root test companion.

- `trusted_logical_root_arrays_keep_declared_metadata_without_conversions`
- `trusted_logical_object_arrays_match_build_and_respect_projection`
- `recorded_validation_selects_trusted_or_validating_extraction`
- `extension_trusted_hook_requires_explicit_capability_in_both_entry_points`
- `registered_reader_expansion_and_failures_keep_typed_classes`

Review this commit as a move. Check the diff for new clones, temporary JSON trees, projections filled by reparsing, registry lookups and extra source reads. The source reader, prepared collection handle, borrowed byte slices and existing owned result values already provide the needed data.

### Commit 2 · Place JSON conversion and consolidate relative-path eligibility

Commit subject `Isolate model JSON conversion and centralize relative-path identity eligibility`.

**Model conversion.** Declare a private `mod json;` under `model/mod.rs`. In `model/json.rs`, place only the existing inherent methods `Value::from_json(serde_json::Value)`, `Number::from_json(&serde_json::Number)` and `Number::parse_json_literal(&str)`, with their original visibility, signatures and bodies. Keep number representation, comparison, hashing, Serde deserialization and finite-number validation with their current model owners. Callers still use the inherent methods. Preserve the exact-decimal fallback, integer and finite-float paths, nested arrays, and the original conversion errors. The JSON document visitor and projected scanner continue consuming record bytes directly.

**Identity.** Add `pub(crate) fn supports_relative_path_identity(&self) -> bool` to the existing `impl SourceFormat`. Its body is `matches!(self, Self::Json | Self::JsonDecodedScalars)`. Call it from these three places.

| Call site | Responsibility retained at the call site |
|---|---|
| `repository/configuration/mod.rs`, `CollectionDefinition::folder` | Select the folder default of `RelativePath` or `Generated`. Allow callers to override it. |
| `construction/batch.rs`, logical record admission | Check an explicitly selected relative-path identity, render the collection-relative native path, retain file and byte-range context in errors. |
| `query_execution/preindex.rs`, `record_id` | Check the same eligibility when a logical record is visited. Keep pre-index path handling and error propagation. |

Search for `IdentityPolicy::RelativePath`, `SourceFormat::JsonDecodedScalars`, and the existing “single-record JSON reader” error. Change another caller only if it implements this same predicate. A configuration loader parsing a format name and a registration choosing a verification operation each perform a different operation. Keep generated IDs based on file ID, revision time and byte start, and fallback IDs based on collection, registered source ID and version, native path, file ID, revision time and byte range. Keep the existing filename and intrinsic-ID paths.

**Reader capability.** Use the existing `PreparedSource::scalar_verification()`. `SourceRegistry::builtin()` records provenance for the registrations it installed. A fresh registry can install a custom implementation named `json`, including one that opts into trusted logical extraction. That opt-in and the built-in scalar-verification capability are independent. Resolve the reader once per collection. Preserve existing public registration, format serialization and configuration hashes.

### Acceptance matrix

Use a nested fixture with an escapable path, two logical JSONL records at different byte offsets, and a malformed input. Exercise explicit `RelativePath` on a **valid logical record** for each supported entry point. A malformed row takes the fallback path and would give a false pass. Assert full generated IDs against the existing ID constructor using observed file metadata, plus byte ranges and classes. Use one indexed and one pre-index run on the same files for built-in registrations.

| Declared format and installed registration | Folder default | Explicit relative path | Resolved scalar operation |
|---|---|---|---|
| `Json`, built-in registry | Relative path | Accepted | `ResolvedStrings` |
| `JsonDecodedScalars`, built-in registry | Relative path | Accepted | `DecodedScalars` |
| `JsonLines`, built-in registry | Generated per record | Rejected | `ResolvedStrings` on the framed record range |
| `Registered("json")`, built-in registry | Generated | Rejected | `ResolvedStrings` |
| `Registered("json")`, custom reader in a fresh registry | Generated | Rejected | None |
| `Registered(custom_id)`, custom non-JSON reader | Generated | Rejected | None |

For a rejected override, assert the error at the existing stage. The construction error includes path and range. Built-in pre-index opening resolves the format, and the logical identity error surfaces during search. The current `SourceScan::open` constructs its own built-in registry. Exercise injected custom readers through indexed construction and the existing registered-reader tests. Compare their indexed/reference results there. Add a small table-driven regression for uncovered cells and reuse the existing API throughout.

| Control | Evidence required |
|---|---|
| JSON, JSONL and fallback | Valid whole-file JSON, projected and complete logical output, exact original bytes, JSONL boundaries, malformed/raw visibility, logical-unavailable diagnostic, and a non-JSON registered reader. Compare ordered IDs, locators, evidence and payloads where applicable. |
| Single-pass verification | Run `decoded_scalar_fts_combines_source_text_with_required_values` in isolation. It checks the combined-scan counter increases, hits equal the reference, counts are 3 and 2 for its two queries, and `record_parses == candidates`. Run `borrowed_source_verification_matches_reference_for_outputs_and_duplicates` and `exact_membership_materializes_only_page_payload` too. Record source reads/opens, bytes and parse counts for the same representative queries at A1 and the candidate, with equivalent worker/cache settings. |
| Validation and resources | All six relocated tests execute. The expansion fixture observes zero reader calls for an oversized estimate and one for a post-parse size rejection or reader failure. Run the registry's built-in-name override test and its non-JSON build/query/restore control. |
| Value and configuration identity | Run the model, JSON document/scalar, typed-parameter coverage and the golden `live_configuration_hash_matches_the_pre_restore_fixture` test. Verify exceptional numeric literals, duplicate object members and projected/skipped content against their existing assertions. |

**JSON-use inventory.** Run `rg -n 'serde_json|SourceFormat::Json|source::json' crates/core/src` and inspect production hits. For each use, record a representative path and the input it handles. Cover record bytes or framed ranges, already parsed model values, schema documents, collection configuration, typed query parameters, workload manifests and field-path string escaping. Identify any diagnostic or persistence encodings found in the search. Identify any remaining record-byte decoder reached outside the resolved source implementation. A generic JSON dependency tally cannot answer that question. Keep current encoding contracts intact.

### Commands and handoff

Run focused tests and `cargo check --locked --workspace --all-targets` for each implementation commit. On the combined source run the following from the integration worktree. Capture actual executed counts. Discover the qualified test name with `-- --list` before using a harness `--exact` selector.

```sh
cargo fmt --all -- --check
python3 scripts/format-rust-spacing.py --check
cargo check --locked --workspace --all-targets --all-features
cargo test --locked -p objekte-search-core --lib
cargo test --locked -p objekte-search-core --lib --all-features
cargo test --locked -p objekte-search-core --test non_json_relations
cargo test --locked -p objekte-search-core --doc
```

Compare A1's default/all-feature unit inventories, including ignored status, against the candidate using the six test moves above. List any added matrix cases separately. Record the reader/identity matrix, JSON ownership inventory, reads and parses, commands, test counts, source SHAs and any unresolved differences. Repeat a failing comparison on A1 to distinguish a pre-existing issue. The full-corpus comparison runs once at the end of Assignment 3.

Append the report to `docs/ai/done/objekte_core_assignment_2_source_preparation.md` under the repository workflow. Commit as `Record source preparation and identity assignment results`, push, and give the owner the full delivery SHA A2. Include B from the Assignment 1 report so Assignment 3 can compare the entire series.

[Original preparation operations](https://github.com/Simon-Martens/objekte/blob/0f1d5e5ffa566a911f926ffe564e49c0c69ded77/crates/core/src/record_preparation.rs) · [Installed reader provenance](https://github.com/Simon-Martens/objekte/blob/0f1d5e5ffa566a911f926ffe564e49c0c69ded77/crates/core/src/source/registry.rs) · [Pre-index search and identity](https://github.com/Simon-Martens/objekte/blob/0f1d5e5ffa566a911f926ffe564e49c0c69ded77/crates/core/src/query_execution/preindex.rs)

## Implementation report

- Task identifier: `core-refactoring-completion-source-preparation`
- Completion date: 2026-11-19
- Series baseline B: `0f1d5e5ffa566a911f926ffe564e49c0c69ded77`
- Entry revision A1: `b4172a1749750ee1c87ef74a56f5ee7cec6c40ee`
- Assignment 2 source C: `c34294489f0011ca7a0009b54e0af5a6d6b600b3`
- Integration branch: `integration/core-refactoring-completion`
- Integration worktree: `/home/simon/source/objekte-integration-core-refactoring-completion`

### Implementation commits

1. `9d2e7ec4f801b93b1ee23c61898a04a6ef64ecc3` — `Assign record preparation to construction, source verification and payload delivery`
2. `c34294489f0011ca7a0009b54e0af5a6d6b600b3` — `Isolate model JSON conversion and centralize relative-path identity eligibility`

Commit 1 removes the old `record_preparation` module. Build validation now belongs to construction, query projection and logical-size accounting to `source::preparation`, combined decoded-scalar verification to `source::verification`, and payload selection to `query_results::payload`. All production callers use those owners. The five query-preparation tests and build-validation test retain their names and bodies at their new module paths.

Commit 2 moves JSON conversion methods into the private `model/json.rs` owner, preserving inherent-method callers. `SourceFormat::supports_relative_path_identity` is now the single predicate used for folder defaults, indexed logical admission and pre-index record IDs. Reader registration remains independent of identity eligibility and scalar verification capability.

### Identity and reader matrix

| Declared format and installed registration | Folder default | Explicit relative path | Evidence |
|---|---|---|---|
| `Json`, built-in | Relative path | Accepted | Indexed/pre-index IDs compare; escapable path renders `path:nested/a%25 b%5C.json` |
| `JsonDecodedScalars`, built-in | Relative path | Accepted | Indexed/pre-index IDs compare on the same fixture |
| `JsonLines`, built-in | Generated | Rejected | Construction includes path/range; pre-index rejects during search |
| `Registered("json")`, built-in | Generated | Rejected | Construction includes path/range; pre-index rejects during search |
| `Registered("json")`, fresh custom reader | Generated | Rejected | Custom trusted reader remains ineligible during construction |
| `Registered(custom_id)`, fresh non-JSON reader | Generated | Rejected | Existing build/query/payload/restore control passes with generated IDs |

The JSONL control also asserts generated IDs from observed file ID, modification time and distinct byte starts, and verifies indexed and pre-index IDs agree. A malformed sibling takes the fallback path, preventing a false positive from malformed-only fixtures.

### Validation

At A1 and the combined candidate, the focused controls passed:

- All six original `record_preparation` tests passed at A1 before their ownership move.
- The five tests under `source::preparation::tests` and the construction validation test passed after the move.
- `decoded_scalar_fts_combines_source_text_with_required_values`, `borrowed_source_verification_matches_reference_for_outputs_and_duplicates` and `exact_membership_materializes_only_page_payload` passed.
- Model tests: 14 passed. JSON document/scalar tests: 38 passed, 2 ignored. Typed parameter tests: 1 passed.
- Folder identity/default matrix, indexed/pre-index identity matrix, registered-reader matrix, built-in-name override and non-JSON build/query/restore controls passed.
- `live_configuration_hash_matches_the_pre_restore_fixture` passed.

Final commands:

| Command | Outcome |
|---|---|
| `cargo fmt --all -- --check` | Passed |
| `python3 scripts/format-rust-spacing.py --check` | Failed at A1 and candidate with the same repository-wide baseline |
| `cargo check --locked --workspace --all-targets --all-features` | Passed |
| `cargo test --locked -p objekte-search-core --lib` | 608 passed, 16 ignored |
| `cargo test --locked -p objekte-search-core --lib --all-features` | 609 passed, 16 ignored |
| `cargo test --locked -p objekte-search-core --test non_json_relations` | 1 passed |
| `cargo test --locked -p objekte-search-core --doc` | 1 passed |
| `cargo clippy --locked --workspace --all-targets --all-features` | Passed |

The spacing check remains a pre-existing failure: A1 lists 198 files and the candidate lists 199, with the candidate difference limited to the new `model/json.rs`, moved preparation test and removed old test path. This was reproduced at A1 with the same command and is not introduced as a new formatting class.

### Source-read comparison

The same four JSON files and absolute root were used at A1 and C with one query worker and zero verified-cache bytes. The three public query executions produced identical IDs, counts and counters at both revisions:

| Query | Count | IDs | Files opened | Distinct files | Bytes read | Record parses | Residual evaluations |
|---|---:|---|---:|---:|---:|---:|---:|
| Exact indexed page | 2 | `00.json` | 1 | 1 | 0 | 1 | 0 |
| `old` with combined filter | 3 | `00`, `01`, `03` | 3 | 6 | 165 | 3 | 3 |
| `1e0` with combined filter | 2 | `00`, `01` | 3 | 5 | 167 | 3 | 3 |

The combined-filter queries also scanned three records, matching their parse counts. The complete probe, fixture generator, compact A1/candidate outputs and assertion script are in [assignment-2-source-preparation](../generated/core-refactoring-completion/assignment-2-source-preparation/README.md). `python3 verify.py` exits unsuccessfully on any changed result.

### JSON ownership

The production inventory is recorded in [json-use-inventory.md](../generated/core-refactoring-completion/assignment-2-source-preparation/json-use-inventory.md). Record bytes and framed ranges are decoded by the resolved source reader in `source/json/document.rs` and `source/json/scalars/mod.rs`. `model/json.rs` handles already parsed values; schema files, collection configuration, typed query parameters and workload manifests retain their separate encoding contracts. Field-path rendering keeps JSON string escaping. No record-byte decoder remains outside the resolved source implementation.

### Unit-test inventories

| Profile | A1 | Candidate | Added matrix tests | Ignored | Ignored status equal |
|---|---:|---:|---:|---:|---|
| Default | 621 | 624 | 3 | 16 | Yes |
| All features | 622 | 625 | 3 | 16 | Yes |

The six moved tests account for no count change. The three added tests are `folder_identity_defaults_and_relative_path_eligibility_follow_format`, `relative_path_identity_matches_indexed_and_preindex_paths`, and `registered_readers_do_not_enable_relative_path_identity`. Every A1 test maps to exactly one candidate test under the specified move, with ignored status preserved.

### Cleanup and handoff

The detached A1 comparison worktree was clean and removed after its test and read results were saved. The temporary Cargo example and fixture directory were removed. The integration worktree and branch are retained for owner review. The full-corpus build/query comparison remains Assignment 3 work.

## Post-review repair results

The identity test module now lives at
`query_execution/preindex/tests.rs`. Its replacement tests independently
specify whole-file ranges, JSONL ranges, classes and identities; validate
native repository-relative paths and exact `0..17`, `18..36` and `37..54`
bounds; check generated and fallback constructors; reject duplicate, missing,
extra and wrong-file locators; and verify logical fallback diagnostics.

Portable percent paths use the platform path encoder. The literal-backslash
assertion remains Unix-only and checks `path:nested/back%5Cslash.json`.
Rejected overrides now isolate one valid file per case and assert the specific
native filename plus exact construction range. Built-in `Registered("json")`
successfully uses generated IDs and `ResolvedStrings`. A fresh custom reader
under `json` opts into trusted extraction while exposing no built-in scalar
operation; declared `Registered("json")` remains ineligible for path identity.
Declared `Json` with a custom multi-record reader retains path eligibility and
fails indexed admission on duplicate IDs.

The [replacement evidence](../generated/core-refactoring-completion/assignment-2-source-preparation/README.md)
now includes robust public-result validation, explicit query start/end markers,
a filtered successful-I/O trace, compact per-query read records and negative
verifier self-tests. The repaired source is
`d07b700a7188efd6b45ec542f197b78a211cb88a`; the final report commit is the
Assignment 2 repair handoff.

Public counters remain inherited observations: `combined_old` reports three
opens, six distinct files and 165 bytes, while the trace records six successful
opens across three paths and 330 bytes. `combined_number` reports three opens,
five distinct files and 167 bytes, while the trace records five opens across
three paths and 286 bytes. Selected payload fields explain the byte
difference. The counter defect is recorded in
[query_source_counter_semantics.md](../issues/bug/query_source_counter_semantics.md);
changing public semantics remains separate maintenance work.

The repaired combined suites pass with 611 default and 612 all-feature tests,
16 ignored in each profile. The review's original 608/609 counts are retained
as its historical execution record. No Windows runner was available; focused
tests and traces ran on Linux. Plain rustfmt requests one blank line around the
two `impl` blocks in `model/json.rs`, while the repository spacing script
accepts the required two blank lines and still rejects its pre-existing
199-file baseline.
