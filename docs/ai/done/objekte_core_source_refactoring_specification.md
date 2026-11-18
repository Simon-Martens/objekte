# Assignment 4 — Source readers, formats and byte access

**Task** `core-source-refactoring`  
**Integration branch** `integration/core-source-refactoring`  
**Sibling worktree** `/home/simon/source/objekte-integration-core-source-refactoring`  
**Starting point** The integrated tree after Assignment 3, the Assignment 2 diagnostic repairs and the interim module cleanup. Both prerequisite branches are included in inspected master `bba4d0a8c3ce1e6a39ce6b04dd0469aedb83b40d`. Fetch and record the actual starting SHA, then use its module paths. The source references below are pinned to `c982344f87` to identify the implementations reviewed.

## Overnight delivery with Assignments 5 and 6

Implement this specification first on `integration/core-source-refactoring` in its own sibling worktree. Read `objekte_core_schema_binding_refactoring_specification.md` for the dependent assignment. Use `docs/ai/README.md` for task records and push every commit. Record the starting master SHA as **B**, run the checks below, commit the completion report, and verify the remote integration head. Record that final immutable SHA as **A4**. If the report is the final documentation-only commit, include the tested code SHA in the report and confirm its source tree matches A4.

Once A4 is pushed and validated, leave the branch at that head. Create the Assignment 5 branch in its own worktree **from A4** and proceed immediately. After 5 passes, use its exact pushed head for Assignment 6 as specified in `objekte_core_lifecycle_coordination_specification.md`. The owner reviews each branch in sequence in the morning. Leave `master` and the original checkout untouched. If a branch already exists, inspect its ancestry and worktree and resume the task there. Preserve any existing work.

The morning comparisons are **B → A4** for sources, **A4 → A5** for schema and binding, and **A5 → A6** for lifecycle coordination. Each task gets its own completion report and test results. If a real blocker remains after in-scope repairs, record the last pushed SHA, the failing command and the dependency affected. Never mark partial work ready.

## Outcome

A contributor adding a reader should find one trait and one registry. A contributor changing JSON decoding should work within the JSON module. Someone changing file-handle reuse or bounded reads should work in source access. The query planner must continue choosing FTS and structured indexes using provider costs and membership. The runner requests source verification only when its existing plan requires it.

Today `source.rs` combines those responsibilities in 2,077 lines, its companion test file has 2,450 lines, and the JSON scalar walker occupies another 1,334 lines at the crate root. This assignment removes seven direct entries from `crates/core/src/`: `source.rs`, `source.test.rs`, `source_access.rs`, `source_access.test.rs`, `json_scalar_text.rs`, `json_scalar_text.test.rs` and `source_scan.rs`. On the planned interim layout, **74 direct Rust files become 67**, including **32** remaining root test companions. Verify the seven paths even if unrelated changes shift the absolute count. [Source implementation](https://github.com/Simon-Martens/objekte/blob/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src/source.rs) · [Scalar walker](https://github.com/Simon-Martens/objekte/blob/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src/json_scalar_text.rs) · [Range session](https://github.com/Simon-Martens/objekte/blob/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src/source_access.rs)

## 1. Assign each implementation one home

Paths are relative to `crates/core/src/`. Define each item at its canonical path. Update maintained imports, including CLI, desktop, examples, integration tests and current rustdoc. Remove obsolete declarations and path aliases in the same compiling change.

| Destination | Move here |
| --- | --- |
| `source/mod.rs` | The public format-independent contract. Define `SourceFormat`, `SourceReader`, `ScanProjection`, `TextSegment`, `ExtractedRecord`, `SourceOutcome`, `ReaderFailure`, `ReaderDiagnostic`, `DiagnosticCategory`, `LogicalTextCoverage` and `TextDerivation` here. This module contains their definitions and trait defaults. |
| `source/registry.rs` | `SourceRegistration`, `PreparedSource`, `SourceRegistry`, built-in registration, and resolution. The canonical public import for the registry is `source::registry::SourceRegistry`. Resolve the reader once per collection. |
| `source/text.rs` | `finalize_extracted_text`, `text_segments`, the logical string/empty-array visitor and its test instrumentation. These operate on the shared logical `Value` and also serve custom readers. |
| `source/json/mod.rs` and `source/json/document.rs` | Define `JsonReader` and `JsonDecodedScalarsReader` and their trait implementations in the parent. Put the serde seed and visitors, logical build deserializer, projected parse functions, JSON diagnostics and reader-failure mapping in `document.rs`. The JSON Lines reader reuses the same single-record parse functions. |
| `source/json/scalars/mod.rs`, `strings.rs`, `scan.rs` | Put `ScalarWalker`, value extraction, projection and complete-document validation in the parent. Put quoted-string scanning, hex/escape/Unicode decoding and delimiter strategies in `strings.rs`. Put the borrowed scanning methods, `CombinedFts`, event handling, worker scratch and fused matching entry points in `scan.rs`. Keep the shared walker state private to this family. Preserve the fused traversal and one worker-local scratch allocation domain. |
| `source/json_lines.rs` and `source/framing.rs` | Define `JsonLinesReader` and its full-file framing in `json_lines.rs`. Put generic range validation, `BoundedJsonLines<R>`, `PendingBuildLine` and `BoundedBuildChunk` in `framing.rs`. The bounded framer is available to any reader explicitly implementing the existing line-framing capability. |
| `source/extraction.rs` | Public extraction callbacks, counters, build samples, timed reads, reusable buffers, optional fingerprinting, `frame_file_for_build`, framed outcome delivery and the running file-coordinate cursor. The construction scheduler continues to choose work units, own concurrency and admit records. |
| `source/access/mod.rs` and `source/access/session.rs` | Put `RecordRangeReader`, single/batched reads, checked opens, file identity, range coalescing and cursor management in the parent. Put `SourceAccessSession`, `RangeBatch`, handle eviction, retained windows and search-side failure classification in `session.rs`. Keep check-only/test-only helpers at their existing visibility and feature gates. |
| `source/verification.rs` | The crate-private interface for selecting and invoking the existing borrowed scalar verification operations. Its responsibility is specified below. The parser and scratch implementation remain in `source/json/scalars/`. |
| `query_execution/preindex.rs` | Move the real `SourceScan` implementation, its helpers and tests. Its canonical public path is `query_execution::preindex::SourceScan`. Preserve the existing inventory-only open and exact scan. The indexed scanner at `query_execution/runner/source_scan.rs` remains a separate caller with its own semantics. |

Declare `source::registry`, `source::json`, `source::json_lines`, `source::text`, `source::extraction` and `source::access` public for their existing public operations. Keep `source::verification`, `source::framing` and `source::access::session` crate-visible, and keep JSON implementation children private except for the narrow crate-internal functions required by verification. Do not add a forwarding `source.rs` alongside the directory.

Keep `ByteRange`, `SourceRevision`, file identity and record-slot types under their existing inventory/model owners. Discovery still selects files. `record_preparation` still applies schema conversions and materializes return modes. Schema/binding and the independent evaluator are Assignment 5. The scope here is the three existing built-in registrations and the present extension contract.

## 2. Select verification from the resolved reader

There is a concrete reason to change more than paths. `DirectSourcePlan::prepare_filter` selects the JSON byte scanner by matching `collection.source.id()`. Several projection and fused-FTS paths repeat that test. A caller can create `SourceRegistry::default()` and register a non-JSON reader under `"json"` or `"json_decoded_scalars"`. The identifier is legitimate in that registry. Sending that reader’s bytes to the built-in JSON walker can produce a parse error, omit reader-owned text or verify a term against the wrong representation. Preserve ordinary custom registration under these names. [Registry resolution](https://github.com/Simon-Martens/objekte/blob/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src/source.rs#L403) · [Direct scanner selection](https://github.com/Simon-Martens/objekte/blob/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src/query_predicate/scan.rs#L325) · [Fused verification](https://github.com/Simon-Martens/objekte/blob/c982344f87c9ff01d1316bba4b9b3d7c5bde53e7/crates/core/src/query_execution/segment_scan.rs#L78)

1. Record the provenance of the **installed registration** when resolving `PreparedSource`. Use its existing private built-in provenance and registration ID at resolution to distinguish ordinary JSON/JSONL, decoded-scalar JSON, and registered implementations. Expose a crate-private `PreparedSource::scalar_verification() -> Option<ScalarVerification>`. Define the copyable `ScalarVerification` variants `ResolvedStrings` and `DecodedScalars`. User registrations return `None`, even when their ID, extension or trait-reported name happens to match a built-in. Maintain registration ID, version, extension, serde spellings, duplicate rejection and configuration fingerprints.
2. At the source boundary, let `ScalarVerification` dispatch the borrowed scalar scan using the existing query inputs and result. Derive the scanner’s decoded/resolved flag from the selected variant. Route the existing combined projection-plus-FTS entry through this boundary for decoded scalars. Move or borrow the existing result and evidence directly. The runner and predicate code should have no imports from `source::json::scalars`. Keep the `SourceReader` trait’s public defaults intact. No new virtual method per record is needed.
3. Replace ID-based **execution** choices in `query_predicate/scan.rs`, `query_execution/runner/{segment_scan,source_scan,relations}.rs` and `relation_query.rs`. Update `record_preparation.rs` at the combined verification call. Keep the existing logical-record, schema-conversion, computed-sort, matcher, quantifier, evidence and field-scope eligibility tests. The existing original-record-bytes matcher for custom readers must remain available when the built-in verifier is absent. ID use in registration, persistence, configuration, diagnostic attribution and publication identity checks still serves those operations.
4. A reader can already declare text coverage, text derivation, trusted query extraction or bounded line framing independently. Continue honoring those declarations at their existing call sites. In particular, a custom reader with complete text coverage can participate in sound FTS/FILTER index selection while using its own parse/verification path. Keep index proposals, cost comparison, domain checks, bitmap operations and residual truth in their present query owners. A format module may call the existing matching routines with a borrowed scalar. It must never inspect a posting tree or choose an index.

## 3. Preserve source semantics and work bounds

| Path | Invariant to verify |
| --- | --- |
| Fresh construction | Consume actual bytes using the existing reusable buffer, optional digest and one parse per logical record on the normal path. Preserve callback order, counters, the 1 MiB retained-buffer limit and the fallback handling of I/O versus malformed data. Keep revision probes off the fresh-build path. |
| JSON and JSON Lines | Preserve relative-path/generated/intrinsic IDs under the existing policies. Preserve blank-line skipping, CRLF, final lines without LF, oversized single records, chunk-relative ranges, absolute offsets and preceding-line counts. Custom frame ranges must be ordered, non-overlapping and in bounds before slicing. |
| Outcome delivery | Malformed records retain their source bytes and diagnostic coordinates for raw fallback. Unreadable files produce the existing file outcome. Resource-limit or fatal reader failures must retain their error classification. Preserve the running coordinate cursor so malformed JSONL does not rescan every preceding line. |
| JSON text | Ordinary JSON/JSONL derive text from resolved logical strings as before. Decoded-scalar JSON retains lexical number tokens and the existing duplicate-member rules for unscoped versus field-selected FTS. Preserve trusted projection of skipped fields, validating parse errors, escape handling, worker-local scratch reuse and cleanup after errors. |
| Source reads | Keep checked byte/identity reads distinct from indexed search reads that trust the recorded revision. Preserve cancellation, source/target record allowances, cursor recovery after failed reads, the eight-handle session limit and 64 KiB lookahead. Bounded streaming reads retain their own cap and skip lookahead. Keep physical byte counters inclusive of coalesced gaps. |

Keep input ownership explicit: pass slices while a buffer is borrowed, move owned chunks/records/results to the current consumer and keep existing shared handles. Moving a module gives no reason to clone a bitmap, copy result evidence, collect scalar events into a second representation, or parse JSON twice. The current owned `RangeBatch` may copy source bytes from its retained window. Preserve that contract in this assignment; a borrowed-window redesign requires consumer lifetime measurements of its own.

## 4. Migrate tests and prove the boundary

Move the three root test companions into module-local suites, retaining every test, feature gate and ignored benchmark. Split the large `source.test.rs` by behavior. Put revision/seek cases under `access/tests.rs`, window and eviction cases under `access/session/tests.rs`, line/chunk cases under `framing/tests.rs`, extraction and coordinate cases under `extraction/tests.rs`, parser cases under `json/tests.rs`, and custom registration/end-to-end cases under `registry/tests.rs`. Move the existing discovery and intrinsic-ID integration cases together with the minimum shared test fixtures. Use a test-only fixture module when multiple suites need `TinyReader` or `OutcomeReader`. Split `json_scalar_text.test.rs` between scalar decoding/projection and borrowed scanning; keep private access and the ignored microbenchmarks with their owners.

**Required new regression.** Parameterize a custom reader under the built-in spellings in a fresh registry. Use non-JSON input and a reader-owned searchable segment and logical field with known, distinct values. Build through `Index::start_build` with that registry. Run a literal FTS query and a residual structured FILTER through a pinned view, checking hit identity, evidence where requested and false cases. Include an indexed term combined with a residual term and compare against the independent evaluation or explicit fixture results. Run the same fixture under a neutral registration ID as a control. Make the FILTER field unindexed so the test really reads through the extension. Use a per-fixture counter if instrumentation is needed. The test should fail on an accidental built-in scanner or an unsound projection; a registry lookup assertion alone is insufficient. For the real built-ins, retain the existing tests that prove direct and fused verification still execute and avoid owned text.

| Evidence | Existing tests to retain and run |
| --- | --- |
| Reader interoperability and planning | `registered_non_json_reader_reaches_build_query_payload_and_restore`, `body_only_reader_does_not_prune_unindexed_title`, `temporal_registration_does_not_replace_reader_owned_text`, `selective_text_reduces_the_input_to_the_structured_lookup`, and the `non_json_relations` integration target. |
| Offsets, fallback, bounded work | `bounded_jsonl_chunks_preserve_whole_file_ranges_and_line_origins`, `seeded_reader_ranges_obey_checked_nonoverlap_contract`, `malformed_jsonl_coordinate_work_grows_with_file_size`, `build_reads_use_actual_bytes_without_revision_probes`. |
| Byte access and pre-index search | `positioned_reader_tracks_coalesced_windows_and_failed_reads`, `bounded_stream_reads_skip_lookahead_and_cap_spaced_range_windows`, `checked_range_reads_reject_replaced_path_with_spoofed_revision`, `source_scan_matches_reference_search_before_an_index_exists`. |
| Allocation and parsing | `fresh_build_decodes_value_and_scalar_text_in_one_walk`, `combined_value_and_source_matching_avoids_owned_segments`, `borrowed_source_worker_scratch_reuses_capacity_across_records`, `oversized_folded_scratch_is_released_on_scan_error`. |

## Commit order and acceptance

1. Start a sibling worktree from integrated master. Record the base SHA, the seven root paths, and the core test inventory from `cargo test --locked -p objekte-search-core -- --list`. First move the public contract, registry, registrations and format-independent text operations. Migrate consumers in the same compiling commit.
2. Move JSON, decoded scalars, JSON Lines and their fixtures. Separate string decoding and borrowed scanning mechanically. Preserve function bodies and hot parser loops wherever possible.
3. Move framing, extraction, range access, session management and pre-index search. Delete the old root declarations. Audit root-file counts, rustdoc links, examples and gated imports. Migrate the remaining test companions by behavior.
4. Make the typed verification change and add the custom-registration regression in a distinct commit. Compare parse, allocation, I/O and candidate-work counters against the base on equivalent built-in queries. Check both normal JSON and decoded scalars. If a parser loop, memory ownership, or I/O schedule changed, record a focused release-mode before/after comparison for that operation on identical inputs. Mechanical moves and a registration check resolved once per collection need no full Musenalm benchmark.
5. Run `cargo fmt --all -- --check`, `cargo check --locked --workspace --all-targets`, `cargo test --locked --workspace`, `cargo test --locked -p objekte-search-core --all-features` and `RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc --locked --workspace --no-deps`. Compare base/head test inventories by test name, accounting for module prefixes. Record any base-only failure separately. Verify that active query/runner code contains neither string-based built-in dispatch nor imports of JSON implementation modules.
6. Follow `docs/ai/README.md`. Push coherent commits on the integration branch. Append the actual symbol/path map, tested SHA, seven-file removal check, test/counter results and any measured change to the completion report. Push after each commit. Deliver the frozen, tested A4 head for morning review and start Assignment 5 on that SHA.

## Implementation report

- **Task:** `core-source-refactoring`
- **Completion date:** 2026-11-19
- **Base:** `1eef51b12c9dad55c2b9b7e603b888412cb51ad5` (B)
- **Integration branch:** `integration/core-source-refactoring`
- **Integration worktree:** `/home/simon/source/objekte-integration-core-source-refactoring`
- **Tested code revision:** `77fc0f04a6`
- **Implementation commits:** `5b4c8ada1a`, `7638fda97b`, `3b8149bec7`, `77fc0f04a6`
- **Status:** complete and ready for review; no merge into `master` was performed

### Summary

The seven former root source modules are now owned by the source module tree. The
format-independent contract remains in `source::*`; registrations, JSON decoding,
decoded-scalar scanning, logical text, framing, extraction, range access, session
management, and verification dispatch have separate canonical homes. JSON Lines
framing is available only through the existing bounded-line capability, and
pre-index search is now `query_execution::preindex::SourceScan`.

Typed verification is selected from the installed registration provenance rather
than the collection format ID. A registration installed by a default/custom
registry under `json` or `json_decoded_scalars` therefore returns no built-in
scalar verifier and remains on its reader-owned parse/verification path. Built-in
JSON and JSON Lines select resolved-string scanning; built-in decoded-scalar JSON
selects decoded-scalar scanning. The new regression builds a non-JSON custom
reader under both built-in spellings and a neutral control ID, exercises FTS,
an unindexed residual FILTER, their intersection, evidence, false cases, and the
independent reference evaluator through `Index::start_build`.

### Symbol and path map

| Canonical path | Contents |
| --- | --- |
| `source/mod.rs` | Contract types and `SourceReader` trait/defaults |
| `source/registry.rs` | `SourceRegistration`, `PreparedSource`, `SourceRegistry`, built-in registrations and typed provenance |
| `source/text.rs` | Logical string extraction, empty-array visitation and test instrumentation |
| `source/json/mod.rs`, `source/json/document.rs` | JSON readers; serde seeds, projected parsing, logical build decoding and diagnostics |
| `source/json/scalars/{mod,strings,scan}.rs` | Scalar walker/projection, quoted-string decoder and borrowed/fused matching; the implementation is split by ownership while retaining worker-local scratch |
| `source/json_lines.rs`, `source/framing.rs` | JSON Lines reader and bounded generic line framing |
| `source/extraction.rs` | Extraction callbacks, counters, timed reads, digesting, reusable buffers and coordinate delivery |
| `source/access/mod.rs`, `source/access/session.rs` | Checked range reads, file identity, coalescing, session windows, handle eviction and search failure classification |
| `source/verification.rs` | Crate-private `ScalarVerification` dispatch over borrowed scanner operations |
| `query_execution/preindex.rs` | Public `SourceScan` |

All maintained core, CLI, desktop, example, integration-test and rustdoc imports
use these canonical paths. Compatibility re-exports for the former root modules
were removed.

### Removed root paths

The required final-state removal check passed for all seven paths:

- `crates/core/src/source.rs`
- `crates/core/src/source.test.rs`
- `crates/core/src/source_access.rs`
- `crates/core/src/source_access.test.rs`
- `crates/core/src/json_scalar_text.rs`
- `crates/core/src/json_scalar_text.test.rs`
- `crates/core/src/source_scan.rs`

The direct Rust-file count at the base was 74. It is 67 at tested revision
`77fc0f04a6`. The former source test companion was split into module-local
registry, JSON, framing, extraction and access/session suites.

### Validation

The following commands passed at `77fc0f04a6`:

```text
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked -p objekte-search-core --all-features
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps
```

Default core results: 583 passed, 16 ignored. All-feature core results: 584
passed, 16 ignored. Workspace CLI, desktop, process integration and doc tests also
passed. The explicitly ignored source microbenchmarks remain registered:
`borrowed_consumer_microbench`, `decoder_strategy_microbench`, and the existing
construction/index profiling tests.

The base/default `cargo test --locked -p objekte-search-core -- --list` inventory
contained 598 unit tests. The head contains 599. Comparing final test function
names after allowing module-prefix changes reports exactly one addition:
`custom_readers_under_builtin_names_never_use_builtin_scalar_verification`.
All integration-test names are unchanged.

The new regression passed exactly:

```text
cargo test --locked -p objekte-search-core --lib \
  source::registry::tests::custom_readers_under_builtin_names_never_use_builtin_scalar_verification \
  -- --exact
```

Existing parse, allocation, I/O and candidate-work tests remain green, including
single-walk build decoding, combined borrowed FTS/FILTER scanning, worker scratch
reuse, oversized-scratch cleanup, bounded reads, coalesced-window accounting and
pre-index/reference parity. No parser loop, ownership policy or I/O schedule was
changed by this mechanical refactor beyond typed verifier selection; therefore no
full Musenalm benchmark was required. Focused tests assert unchanged parse counts
and zero-copy/borrowed paths.

### Review notes

No unresolved issue or blocked check remains. The reviewed boundary is the final
documentation-only commit after tested code revision `77fc0f04a6`; its source tree
matches that tested revision. The integration worktree and branch are retained for
the owner’s merge decision.
