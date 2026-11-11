# Objekte query preparation repairs

Implement only the following two repairs. Reviewed revision: `5166876c7228f4fe2f8f064861cdc8a88275ad18`; check the current code before changing it.

**Work directly on `master`.** The user explicitly authorizes this exception to the usual integration-branch/worktree workflow. Preserve unrelated changes. Use one agent and two small commits; push each commit normally after its relevant checks pass.

## 1. Preserve declared array metadata

In `crates/core/src/reference.rs`, `convert_indexed_record()` skips schemas without ID or temporal conversions. This loses array annotations previously applied when returning logical payloads.

For example, `{"values":[]}` with a declared `Array<Number>` must retain `element_type: Some(Number)` and the declared element nullability.

- Apply declared array annotations alongside necessary value conversions, without repeating schema validation.
- Cover root arrays, nested arrays, and arrays in objects, including schemas with no ID/date/time fields.
- Respect projections and preserve undeclared arrays' existing inferred metadata.
- Keep ordinary scalar schemas cheap: traverse only schema paths needing conversion or annotation. Do not restore required-field, type, or nullability checks during trusted search.

Add focused tests comparing build-prepared values with trusted logical payloads for empty, all-null, populated, and nested declared arrays. Include declared nullable elements with no null actually present.

## 2. Enable explicit extension trusted parsing

In `record_preparation.rs`, both query-preparation entry points exclude all extensions from trusted dispatch, making an extension's `SourceReader::parse_for_query_trusted()` override unreachable.

- Replace that blanket exclusion with an explicit reader capability. Reuse an existing suitable capability; otherwise add a default-false trait method.
- Opt-in means successful build parsing validates the complete source record and trusted query extraction is supported. Built-in JSON readers opt in; extensions can opt in explicitly.
- For indexed `Logical` records from an opted-in reader, call the trusted hook and conversion/annotation-only schema path. Other extensions keep validating behavior.
- Keep raw fallback and unindexed behavior unchanged. Retain extension resource limits and error classification independently of trusted dispatch.
- Apply the same decision in both preparation entry points. Keep trait defaults compatible and document that a changed validation contract requires an explicit rebuild.

Test with instrumented extension readers: an opted-in indexed reader calls only its trusted hook; a default reader uses validating parsing. Cover both entry points and preserve existing resource-limit tests.

## Completion

Search must gain no freshness checks, source hashing, or revalidation of trusted records. Do not include borrowed-segment optimization or unrelated refactoring.

Run focused regression tests, the core test suite, and repository-required formatting/checks. No new benchmark campaign is required. Report the two commit IDs, checks run, and any remaining limitation.


## Implementation report

Task identifier: `query-preparation-repair`. Completed 11 November 2026. Base commit: `dfde3511f981` (the merged borrowed-segment follow-up). The user-approved branch/worktree exception was followed: implementation took place directly on `master` in `/home/simon/source/objekte`, with one agent and two commits. No task-specific integration or subordinate worktree was created.

The first repair makes trusted indexed-value preparation visit declared array paths as well as ID and temporal conversion paths. It restores declared element type and nullability for root, nested, and object arrays, respects top-level projections, and leaves undeclared inferred arrays intact. Scalar-only schemas still return without traversal, and trusted preparation does not reinstate type, required-field, or nullability validation. Commit: `be763c6c1394` (`fix(core): preserve declared arrays in trusted query values`).

The second repair adds `SourceReader::supports_trusted_query_extraction()`, defaulting to false. Built-in JSON readers opt in; extension readers may opt in only when successful build parsing validates the complete source record and their trusted query hook supports extraction without revalidation. Both query-preparation entry points use this capability for indexed `Logical` records. The execution-aware entry point still applies extension estimate/post-parse resource limits and extension error classification. Raw fallback behavior and the recorded-validation consistency policy remain unchanged. The extension implementation and this report are in the second task commit; its identifier is reported after the push, rather than included in its own contents.

Validation: focused array, registration-order, reader-dispatch, stale-index, and extension resource/error tests passed. `cargo test --locked -p objekte-search-core --lib` passed after the first repair (488 passed, 12 ignored). `cargo test --workspace --locked` passed after both repairs (489 core library tests passed, 12 ignored, plus the CLI, desktop, integration, and documentation suites). `cargo clippy --locked --workspace --all-targets` passed with existing warnings. All five task-owned Rust files match rustfmt plus the repository spacing convention, and `git diff --check` passed. The repository-wide `python3 scripts/format-rust-spacing.py --check` still fails on unrelated existing formatting, including `crates/cli/src/execution.rs`; no unrelated files were reformatted.

No new benchmark campaign was run, as the task specification explicitly excludes one. Extension readers remain on validating query parsing by default; an extension that changes its build-validation contract must opt in deliberately and explicitly rebuild its index. No unresolved in-scope issue remains. The source task document began untracked and was copied byte-for-byte into this completed document before the report was appended; its obsolete source copies are removed only after this document is pushed.
