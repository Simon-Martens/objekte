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


## Implementation report: metadata column repair

Task: `desktop-metadata-filters`, reopened after the user reported that metadata columns displayed `Unavailable`. Completion date: 11 November 2026. Repair base: master `64f3cef393e475788629ead883cb77230d1d8d5d`. Existing integration branch/worktree reused: `integration/desktop-metadata-filters`, `/home/simon/source/objekte-integration-desktop-metadata-filters`. This tracked completion document was clean before the repair; the original specification and report remain above.

Repair implementation: `95008a0c5978e6b33e618cb0e9283a955787947b` (pushed).

Cause: `initial_columns` also consumes the shared field-choice provider. Adding metadata suggestions made those fields available as columns, but `project_field` explicitly rejected every non-data path. The earlier validation covered suggestions and builder values without covering metadata table rendering.

The table now projects all 13 canonical metadata fields directly from captured `SearchHit` values, with the repository root captured once per `QueryPage`. Both indexed and initial source-scan page constructors supply the root. Metadata remains visible and independent of the JSON payload. No per-cell filesystem reads are added; a deleted or changed source cannot silently replace the displayed page's captured metadata. Quoted JSON keys that resemble metadata remain data paths. Byte values and status/class codes retain their numeric forms; timestamps use the shared CLI formatter for Unix epoch milliseconds; absent creation times display `null`. The GUI guide documents these representations.

Validation on the same Linux/rustc/cargo environment recorded above:

- A new page-projection regression test reproduced the bug before the implementation: `cargo test --locked -p objekte-desktop metadata_columns_show_captured_values_without_a_json_payload` failed on `@record.id`, showing `Unavailable` instead of the captured ID.
- `cargo test --locked -p objekte-desktop controller::projection`: passed after the fix (10 projection tests at that point).
- Four new regression tests cover every metadata column with/without a JSON payload, null creation dates, signed timestamp formatting, escaped paths, quoted data keys, a disappeared source, and the source-scan worker's metadata/root projection.
- `cargo test --locked --workspace`: passed once on the repaired implementation. Other parallel desktop/workspace attempts failed only the unchanged scheduler test described below; all metadata tests passed.
- Final `cargo test --locked --workspace -- --test-threads=1`: passed (desktop 167 passed, 1 ignored; core unit tests 503 passed, 15 ignored; CLI unit tests 78 passed; CLI command tests 32 passed, 1 ignored; other integration/documentation tests passed). Desktop package artifacts were cleaned and rebuilt after baseline experiments to ensure this final run used the repaired source tree.
- Staged diff review and `git diff --check`: passed.

An unrelated intermittent scheduler retirement assertion was reproduced on unmodified master, after a clean desktop package build: 20 isolated runs passed, while the baseline full desktop suite failed on repetition 7. The separate issue [desktop_scheduler_test_retirement_race.md](../issues/bug/desktop_scheduler_test_retirement_race.md) records the evidence and distinguishes the observed test failure from the worker-order hypothesis. No scheduler behavior or existing scheduler test was changed. This remains a baseline validation limitation, outside the metadata repair.

No performance benchmark applies to this bounded desktop formatting repair: it reuses result metadata and adds no query execution, source scanning, source IO, or index work. No dependencies or core interfaces changed. Native GUI interaction was not performed; the exact table page projection and both page-production paths were tested. No in-scope failures remain. One supervising agent performed the repair, with no subordinate branches/worktrees. The integration worktree is retained; temporary repair memory is removed after the report push. The user's preceding master-merge instruction remains the delivery scope for this repair.
