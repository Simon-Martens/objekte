# Desktop metadata filters and byte-size builder values

Task identifier: `desktop-metadata-filters`.

User request (11 November 2026): add desktop filter autocompletion for `@record`, `@file`, and `@collection` metadata, and accept size units such as `23MB`, `2KB`, and `02GiB` in the builder value field.

Scope: expose all existing built-in metadata paths with their scalar types in both the text completion and condition builder; reuse existing core byte-size parsing; retain the numeric value field boundary; cover the requested examples and invalid values with desktop regression tests. No core grammar or query execution changes.

Base revision: `05c00a3cbfa2f218ed063681eccbd9769386aef5`.
Integration branch: `integration/desktop-metadata-filters`.
Integration worktree: `/home/simon/source/objekte-integration-desktop-metadata-filters`.

## Implementation report

Task: `desktop-metadata-filters`. Completed 11 November 2026.
Base: `05c00a3cbfa2f218ed063681eccbd9769386aef5`.
Integration branch: `integration/desktop-metadata-filters`.
Implementation commit: `6ce4c51c8671ed28a0598aafa74f74f23b264737` (pushed).

The shared desktop field-choice provider now includes all 13 built-in metadata fields from `MetadataField`, independently of the selected collection, schema, or visible columns. Both the filter text autocomplete and condition builder consume these choices. Numeric metadata uses Number controls, including file sizes, byte offsets/lengths, file IDs, status codes, and record class codes. Text/ID fields and timestamp text use String controls. Repeated visible metadata columns do not duplicate suggestions or erase their known types.

Numeric builder values now use the existing structured query parser, allowing `23MB`, `2KB`, `02GiB`, and fractional sizes that resolve to whole bytes. The builder retains the unit token when publishing the filter; parsing a filter back into builder rows resolves it to the exact integer byte count. Validation requires a single numeric literal, rejecting extra conditions, sorting, parameter/field operands, invalid units, negative byte sizes, fractional bytes, and overflow. Plain numeric and string controls retain their existing behavior. No core grammar or execution changes were needed. The GUI environment guide documents the syntax and decimal/binary unit meanings.

One agent implemented and reviewed the change; no subordinate branches or worktrees were used. The task specification was created untracked at `docs/ai/tasks/desktop_metadata_filters.md` in the integration worktree, then moved here unchanged with this report appended.

Validation on Linux 7.2.15-300.fc45.x86_64 with rustc/cargo 1.99.1:

- `cargo test --locked -p objekte-desktop`: passed during implementation (162 passed, 1 ignored before the final completion test was added).
- `cargo test --locked --workspace`: passed on the final implementation. Desktop: 163 passed, 1 ignored; core unit tests: 503 passed, 15 ignored; CLI unit tests: 78 passed; CLI command tests: 32 passed, 1 ignored. Other integration tests and documentation tests passed.
- Regression coverage checks metadata field types/deduplication and availability across scopes, caret context for metadata filters and sorting, desktop validation of metadata queries, exact size conversion including leading zeros, builder round trips, and invalid numeric values.
- `git diff --check` and staged diff review: passed.

No performance benchmark was run: this is a bounded desktop suggestion/condition serialization change with no changes to source scanning, indexes, or query execution. No native GUI interaction or screenshot acceptance was performed; the completion context, shared field provider, builder serialization, and desktop query validation were verified through tests. No outstanding in-scope failures, approved deviations, or issue reports.

Integration worktree retained for review: `/home/simon/source/objekte-integration-desktop-metadata-filters`. No subordinate worktrees need cleanup. Task memory is removed after this report is committed and pushed.
