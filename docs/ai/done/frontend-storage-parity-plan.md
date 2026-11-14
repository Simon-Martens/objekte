# Agent specification: TUI and desktop storage-status parity

Task: `frontend-storage-parity` (follow-up to the closed `hybrid-index-storage` task)
Repository: `Simon-Martens/objekte`
Plan date: 14 November 2026
Reviewed hybrid integration head: `efcae0bb01644c02b49037a49a7dc6dced9c7a82`
Reviewed implementation report: `docs/ai/done/objekte_hybrid_index_storage_implementation_plan.md`

## 1. Objective

Bring the TUI and native desktop application to full parity with the implemented hybrid index storage model:

1. Every storage option is selectable and correctly validated in both frontends: memory-only, system temporary, repository, custom base; on-demand and fully resident disk access.
2. Both frontends display the complete user-relevant storage state: effective location and path, residency, readiness, source reconciliation, current/durable generation, synchronization, validation-stamp status, managed directory, and a classified last error.
3. Both frontends provide a way to persist pending changes on demand and to retry a failed synchronization without closing the application.
4. Changing storage/residency for an open repository is a controlled, explicit flow that preserves or clearly warns about current tabs/query state and never silently abandons unflushed work.
5. Error messages distinguish storage access, corrupt data, incompatible configuration, publication conflict, and resource limits, with actionable guidance. A permission failure must not read as corruption, and no error may trigger automatic deletion.
6. Documentation and help describe the storage modes, residency trade-off, background synchronization, and flush-on-close behavior.

This is a UI/status and presentation task. It does not change storage semantics, index formats, query behavior, or defaults. Memory-only remains the default. Do not merge into `master`.

Non-goals:

- Do not add retention/GC tuning controls, index-size budgets, or new storage locations.
- Do not expose low-level codec terminology (segments, dictionaries, manifests) in primary controls. Detailed diagnostics may show store paths and counts.
- Do not persist a non-memory storage choice across launches by default. Silently reopening a disk-synchronized store on a later launch is a behavior change and a surprise. Preserve memory-only as the startup default; optionally remember only the custom base path if the user asks.
- Do not redesign the TUI navigation or desktop visual system.
- Do not implement the deferred failure-matrix audit from `docs/ai/issues/feat/hybrid-store-failure-validation-coverage.md`; this task only surfaces the categories the core already distinguishes and adds the small status mapping needed.
- Do not rename CLI NDJSON fields or bump the output protocol in this task unless a separate compatibility decision is made.

## 2. Base revision and workflow

Start by recording the actual base. The reviewed hybrid integration head is `efcae0bb`; confirm whether the user has merged `integration/hybrid-index-storage` into `master` by the time work starts.

- If hybrid storage is still unmerged: base the new task on `integration/hybrid-index-storage` at its current head.
- If it is merged: base on the merge commit on `master`.
- Do not branch from the old `da8fde68` master revision; it lacks the storage APIs and frontend controls.

Workflow:

- New task branch: `integration/frontend-storage-parity`.
- Worktree: `/home/simon/source/objekte-integration-frontend-storage-parity`, a sibling of the original checkout.
- Keep `/home/simon/source/objekte` on `master`.
- Read `AGENTS.md`, `docs/ai/README.md`, `docs/development/hybrid_index_storage.md`, `docs/tui.md`, and the closed task report before editing.
- Use one supervising implementer; TUI and desktop can be separate subtasks only if they do not edit the same files:
  - `task/frontend-storage-parity/tui` for `crates/cli/src/tui/**`.
  - `task/frontend-storage-parity/desktop` for `crates/desktop/src/**`.
  - Shared status/presentation files are owned by the supervisor and merged first.
- Push every commit immediately. Run the workspace tests and Clippy before each push.
- Do not force-push or modify remotes.

## 3. Current state

The hybrid storage API is already exposed in the core and both frontends. The following exists at `efcae0bb`.

### Core

- `IndexStoragePolicy { location, residency }` with `MemoryOnly`, `SystemTemporary`, `RepositoryDirectory`, `CustomDirectory`, and `OnDemand`/`FullyResident`.
- `IndexStorageStatus` fields: `readiness`, `source_reconciliation_complete`, `current_generation`, `durable_generation`, `synchronization`, `last_error`, `managed_directory`, `segments_full_validated`, `segments_stamp_trusted`, `validation_stamps_written`, `validation_stamp_error`.
- Operations: `flush_storage`, `attach_storage`, `set_sync_storage`, `stop_sync_storage`, `quiesce`, `wait_for_reconciliation`, `storage_status`.
- Typed `IndexError` variants and internal `StoreReadError` categories, but `IndexStorageStatus.last_error` is only a string.
- Managed store generation layout, validation stamps, and background synchronization are implemented and documented.

### CLI

- `--index-storage memory|temporary|repository|custom`, `--index-directory`, `--index-residency on-demand|full`.
- NDJSON `index_storage_status` includes every status field, including validation stamps.
- Finite disk commands flush before success; memory mode does not touch a store.
- `docs/cli/obj.md` documents the options and behavior.

### TUI

- `Ctrl+K` → **Index storage** overlay: Memory, Temporary, Repository, Custom; custom base input; **Keep entire index in memory** checkbox; **Apply and reopen**.
- Applying a changed policy returns a `TuiReopenRequest` and the CLI reopens with the new policy. Search and filter text are preserved.
- The status line shows readiness, reconciliation/synchronization, current generation and durable generation. `last_error` is appended as an index-synchronization error.
- On orderly quit, disk modes wait for reconciliation, quiesce, and call `flush_storage`.
- Gaps:
  - No validation-stamp counters or `validation_stamp_error` in the status.
  - No managed directory or effective custom path in the status.
  - No manual "Synchronize now" or "Retry synchronization" action.
  - The storage overlay does not show the current effective state or explain on-demand versus fully resident.
  - The custom-base row is awkward: Enter on it moves the cursor to Apply rather than entering edit mode.
  - The storage policy is not persisted; next launch follows CLI arguments. That is acceptable if documented, but the TUI does not say so.

### Desktop

- Open dialog: four storage choices; custom folder input with Browse; **Keep entire index in memory** checkbox; explanatory summary; all choices route through the shared policy.
- Index panel: location, residency, synchronization, managed folder, **Change storage…**, rebuild/compact, automatic checking.
- Model collects `segments_full_validated`, `segments_stamp_trusted`, `validation_stamps_written`, `validation_stamp_error`.
- Close with a managed store quiesces and flushes; close failure offers Retry/Cancel.
- Gaps:
  - The validation-stamp fields are assigned in `controller/runtime.rs` but are **not rendered anywhere**. They are dead presentation state.
  - `IndexUiStatus` has no explicit `current_generation`, `durable_generation`, `source_reconciliation_complete`, or readiness field; some of this is embedded in English strings and cannot be tested or laid out.
  - No manual "Synchronize now" or "Retry synchronization" action; a failed background sync can only be retried by closing or reopening.
  - "Change storage…" reopens the generic **Open data** dialog rather than a focused storage change. It is unclear whether current tabs/queries are preserved, and the user must re-confirm the source.
  - Custom base validation only checks non-empty. It does not canonicalize, require an existing directory, or explain that relative paths resolve against the process working directory. TUI validates and canonicalizes.
  - Desktop Help does not mention storage modes, residency, background sync, or flush-on-close.
  - `last_error` is rendered as an unstructured string; a permission failure can look like corruption.

### Parity matrix

| Capability | CLI | TUI | Desktop | Required |
| --- | --- | --- | --- | --- |
| Four storage locations | yes | yes | yes | yes |
| Custom base selection | flag + core validation | text + canonicalize | text + Browse, weak validation | same validation everywhere |
| On-demand / fully resident | flags | toggle | checkbox | yes |
| Effective location + path | status JSON | partial | location label, managed folder | yes |
| Readiness | status JSON | yes | string only | explicit |
| Source reconciliation | status JSON | combined | combined | explicit |
| Current/durable generation | status JSON | yes | embedded in sync string | explicit |
| Validation stamps | status JSON | no | collected, not rendered | yes |
| Managed directory | status JSON | no | yes | yes |
| Classified last error | string | string | string | typed category |
| Synchronize/retry now | flush at end | no | no | yes |
| Change storage flow | relaunch | dedicated overlay + reopen | generic Open dialog | focused flow |
| Preserve query state on change | n/a | search/filter preserved | unclear | preserve or warn |
| Help/docs | `docs/cli/obj.md` | `docs/tui.md` | desktop help | complete |

## 4. Core status changes

Add the minimum typed information the frontends need. Do not expose internal structs.

### 4.1 Classified storage failure

In `crates/core/src/index_storage.rs`:

```rust
/// User-facing classification of the last persistent-storage failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexStorageFailureKind {
    /// Permission, missing path, disk full, or other I/O failure.
    Access,
    /// Manifest, catalog, segment, dictionary, or posting bytes are invalid.
    Corrupt,
    /// Store format, model, configuration, or validator identity is incompatible.
    Incompatible,
    /// Another writer holds the store lock or a generation changed concurrently.
    Conflict,
    /// A configured resource limit was exceeded.
    Resource,
    /// Anything not classified above.
    Other,
}
```

Add to `IndexStorageStatus`:

```rust
pub last_error_kind: Option<IndexStorageFailureKind>,
pub validation_stamp_error_kind: Option<IndexStorageFailureKind>,
```

Implementation notes:

- `ManagedPersistence.last_error` currently stores only `String`. Add a `last_error_kind` beside it, set when a synchronization/flush error is captured, and clear both on success.
- Map from `IndexError` (and, where available, `StoreReadError`) to the category:
  - `Io` → `Access`
  - `InvalidCache` → `Corrupt`
  - `IncompatibleConfiguration` → `Incompatible`
  - `StoreConflict` / revision/file conflicts at the store boundary → `Conflict`
  - `ResourceLimit` → `Resource`
  - everything else → `Other`
- Keep the existing string fields for compatibility and detailed diagnostics. Do not remove or rename them.
- Validation stamp write failures are warnings, not fatal opens; the kind lets the UI show "Index validated; progress stamp could not be saved" rather than a generic failure.
- Add unit tests for every mapping and for clearing on success.

### 4.2 Shared presentation helper

Both frontends currently build English labels independently. Add a pure helper in the CLI library so the TUI and desktop cannot drift:

`crates/cli/src/storage_view.rs` (exported from `objekte_cli::storage_view`):

```rust
pub struct StorageStatusView {
    pub location: String,          // "Memory only", "System temporary folder", ...
    pub residency: String,         // "On-demand", "Entire index in memory"
    pub readiness: String,
    pub reconciliation: String,
    pub synchronization: String,
    pub generations: String,
    pub validation: String,
    pub managed_directory: Option<String>,
    pub issue: Option<StorageIssueView>,
}

pub struct StorageIssueView {
    pub kind: IndexStorageFailureKind,
    pub summary: String,           // short, user-facing
    pub guidance: String,          // "Retry", "Choose another folder", "Rebuild index", ...
    pub detail: String,            // original error text
}

pub fn describe(
    policy: &IndexStoragePolicy,
    status: &IndexStorageStatus,
) -> StorageStatusView;
```

Rules:

- Pure function, no filesystem access, no allocation-heavy formatting per frame; the TUI may call it only when a status value changes.
- Memory-only: synchronization is "No index files are read or written"; no managed directory; validation is not shown as a failure.
- Disk: include the resolved managed directory and current/durable generations.
- Failure guidance by category:
  - `Access` → "Check folder permissions or free space, then retry."
  - `Corrupt` → "The index data is damaged. Rebuild the index; source files are untouched."
  - `Incompatible` → "The stored index was built with an incompatible format or configuration. Rebuild the index."
  - `Conflict` → "Another process is updating this index. Retry in a moment."
  - `Resource` → "A storage resource limit was reached. Free space or reduce the index size, then retry."
  - `Other` → "Retry; if it persists, use Details to report the error."
- Never suggest deleting a store automatically. Rebuild is the explicit recovery action, and it must not delete source files.
- Add tests using synthetic `IndexStorageStatus` values for all categories, memory-only, pending/synchronized/failed, validation warning, and long paths.

The CLI NDJSON output does not need to change, but adding `last_error_kind`/`validation_stamp_error_kind` to the existing status object is useful. Do not bump the protocol version without a separate decision; additive fields are acceptable if the CLI docs note them.

## 5. TUI requirements

Files: `crates/cli/src/tui/{app,mod,view,settings}.rs`, `crates/cli/src/tui/view/mod.rs`, and new tests.

### 5.1 Storage status

1. Fetch every status field when a managed index is active, not only readiness/sync/generations.
2. Use `storage_view::describe` to format:
   - effective place: "Memory only" / "Temporary: <path>" / "Repository: <path>" / "Custom: <path>"
   - residency
   - readiness
   - reconciliation: "Checking source files" or "Source files current"
   - synchronization
   - generations: "Generation N (durable M)" or "Not yet durable"
   - validation: "Segments: X fully validated, Y trusted stamps, Z stamps written" and a warning line when `validation_stamp_error` is set
   - managed directory
3. Keep the status line bounded. Put the full details in the storage overlay and show a one-line summary in the bottom status area. Do not truncate the error category or guidance away; use a modal/detail row for full detail.
4. Update only when the rendered status view changes, to avoid needless terminal redraws. The existing `last_storage_state` mechanism can compare the new struct or a small digest.

### 5.2 Storage overlay

Extend the existing `Ctrl+K` → **Index storage** overlay; do not invent new navigation.

- Show a read-only "Current" section at the top: location (with resolved path for custom), residency, readiness, sync, generations, validation summary, managed directory, and the classified issue with guidance.
- Options:
  - Memory
  - System temporary folder
  - Repository folder
  - Custom folder (text input; Enter enters edit mode rather than jumping to Apply)
  - Keep entire index in memory (disabled and checked for Memory)
  - Apply and reopen
- Explain the trade-off in one line each: on-demand reads index data from disk when needed; fully resident copies the required index bytes into memory before readiness; memory writes no index files.
- Validate the custom base before applying: trim, expand `~`? (Do not invent shell expansion; accept absolute paths or document relative-to-CWD), canonicalize, require an existing directory, and reject a path inside a selected source directory when the core rejects it. Show the canonical path.
- If unchanged, "Apply" is a no-op; if changed, state that the current view will reopen. Existing search/filter preservation stays.
- Before reopening a disk store, flush pending writes (the current quit path already does). If the flush fails, keep the current view and show the classified error; do not proceed to the new store silently.
- Add a "Synchronize now" / "Retry synchronization" action for disk stores:
  - Visible when `synchronization` is Pending, Failed, or there are unsaved changes.
  - Runs `flush_storage` off the UI thread and reports progress/result; never block the render loop.
  - Success updates the status; failure keeps the previous view and shows the classified error and guidance.
  - Disabled for memory-only with the explanation that no index files exist.

### 5.3 Help/docs

- F1 help must mention `Ctrl+K` → Index storage and the four choices, residency, synchronize/retry, and flush-on-quit.
- `docs/tui.md` must describe the status fields, the overlay, the custom base rules, and the fact that the policy is not persisted across launches.

### 5.4 Tests

- Overlay navigation: Up/Down/Tab/BackTab, selecting each location, toggling residency (ignored/checked for Memory), editing custom path, Enter behavior, Apply unchanged and changed.
- Custom path validation: empty, non-existent, file, relative path, nested source path.
- Status formatting: all readiness/sync/generation/validation combinations; long paths; failure kinds.
- Reopen request: search/filter preserved; flush failure cancels the reopen.
- Synchronize action: pending, success, failure, retry; memory disabled.
- No status polling when memory-only.

## 6. Desktop requirements

Files: `crates/desktop/src/{ui,controller,backend}/**`, plus tests.

### 6.1 Index status model

Extend `IndexUiStatus` with explicit fields rather than parsing English strings:

```rust
pub readiness: String,
pub source_reconciliation: String,
pub current_generation: u64,
pub durable_generation: Option<u64>,
pub validation_summary: String,
pub issue: Option<StorageIssueView>,   // or flattened kind/summary/guidance/detail
```

Populate them in `controller/runtime.rs` from `IndexStorageStatus` and `storage_view::describe`. Keep `storage_location`, `residency`, `synchronization`, `managed_directory` for rendering, but derive them from the same view.

### 6.2 Index panel

Render, in the existing Index storage section:

- Location (with resolved path for custom)
- Residency
- Readiness
- Source reconciliation
- Current generation / durable generation
- Synchronization (including the last failure)
- Validation: segments fully validated, trusted stamps, stamps written; show a warning row when `validation_stamp_error` is present
- Managed folder
- Classified issue with guidance

Requirements:

- Memory-only shows "No index files are read or written" and hides generation/validation rows that cannot apply.
- Full-residency shows "Preparing full residency" until ready, then "Entire index in memory".
- The panel must not reflow or jump on every status poll; keep stable rows and update text in place.
- Do not expose "manifest", "segment reader", "dictionary", or "validator version" in the main label. Detailed diagnostics may show them.

### 6.3 Diagnostics panel

Add a "Storage" section to the existing Diagnostics panel with the full status, the managed path, generations, validation counts, and the raw last error. This is the place for support details; the index panel stays concise.

### 6.4 Synchronize now / Retry

Add:

- `UiAction::SynchronizeStorage` (or `FlushStorage`).
- `IndexTaskKind::Synchronize` for the task summary/cancel state if the existing kinds do not fit.
- Controller worker calls `Index::flush_storage` on the active session, with the same generation guards as other tasks.
- UI button in the Index storage section: "Synchronize now" when pending, "Retry synchronization" when failed, disabled when memory-only or a task is already running.
- Preserve the previous durable generation on failure; show the classified error and guidance. Do not close or rebuild automatically.
- After success, refresh status and show "Synchronized generation N".
- Add tests for success, failure, memory-only disabled, double-click/reentrancy, and close-during-sync.

### 6.5 Change storage flow

Replace the generic Open dialog invocation from **Change storage…** with a focused "Index storage" dialog:

- Prefill the current repository root(s) and current storage policy.
- Show the source root read-only; this is a storage change, not a source change.
- Allow the four locations, custom base Browse, and the residency checkbox.
- Validate the custom base before accepting (existing absolute directory; show canonical path; same rules as TUI).
- State exactly what happens: "This reopens the repository with the new storage setting. Current tabs and queries will be restored where possible; unsaved disk changes are flushed first."
- Preserve current tabs/queries if the controller can reopen the same root without discarding them; otherwise ask for confirmation and explicitly list that tabs will close. Do not silently discard.
- Keep the current session usable while the replacement prepares where feasible; if not feasible, flush before closing it.
- On failure, keep the current session and show the classified error; do not switch the policy in the UI as if it succeeded.
- Cancel restores the previous policy and closes the dialog.
- Tests: memory→temporary, temporary→repository, repository→custom, custom base invalid, cancel, failure, tab preservation/warning, flush failure.

### 6.6 Help/docs

Desktop Help must add a storage paragraph covering:

- memory-only default and no index writes;
- temporary/repository/custom locations;
- on-demand versus fully resident;
- background synchronization, Synchronize now/Retry, and flush on close;
- that source records, schemas, and saved queries are never deleted;
- where the managed folder is shown.

Update any desktop user documentation in `docs/development/` that describes opening repositories.

### 6.7 Tests

- Model: `OpenDraft::storage_policy` for all modes, empty custom, non-existent custom, relative path, memory + full residency, memory + on-demand rejection.
- Controller: status mapping for all readiness/sync/validation/error categories; task state transitions; sync failure retains durable generation and session.
- UI render tests: index panel labels/hidden rows for memory and disk, diagnostics storage section, change-storage dialog prefilled and validation errors, help text.
- Native smoke (documented commands/screenshots): open memory/temp/repo/custom, on-demand and full, close/reopen, failed sync on a read-only store, corrupt/incompatible store guidance, change storage on an open repository.

## 7. Failure behavior contract

Both frontends must present the same categories and never mislead:

| Kind | UI label | Guidance | Automatic action |
| --- | --- | --- | --- |
| Access | Storage access error | Check permissions/free space, retry | None |
| Corrupt | Index data is damaged | Rebuild index; source files untouched | None |
| Incompatible | Index format/configuration changed | Rebuild index | None |
| Conflict | Index is busy | Another process is updating it; retry | None |
| Resource | Storage resource limit | Free space/reduce index size; retry | None |
| Other | Index storage error | Retry; open Details | None |

Rules:

- Never delete a store, source file, schema, `.objekte/collections`, saved query, or unrelated file from a UI error path.
- "Rebuild index" is the existing explicit action and must remain available for `Corrupt` and `Incompatible`.
- A failed flush must leave the old durable generation readable and the current in-memory view usable.
- A validation-stamp write failure is a warning: the index is valid, but the next open may validate more slowly. Show it in validation details, not as a catastrophic error.
- Memory-only never shows storage failures, retries, generations, or managed paths.

## 8. Documentation

Update:

- `docs/tui.md`: storage overlay, status fields, synchronize/retry, custom-base rules, flush-on-quit, no policy persistence.
- `docs/cli/obj.md`: add `last_error_kind`/`validation_stamp_error_kind` if exposed; confirm the four modes and residency text.
- Desktop help and any `docs/development/` opening documentation.
- `docs/development/hybrid_index_storage.md`: short "Frontend behavior" subsection naming the shared presentation helper, the failure categories, and the requirement that memory-only shows no storage state.
- Task completion report under `docs/ai/generated/` or the benchmark result tree if UI evidence is captured.

## 9. Verification

Run:

- `cargo test --workspace --locked`
- `cargo clippy --workspace --all-targets --locked`
- CLI process tests, including TUI tests where available.
- Native desktop `environment_validation` and the documented GUI smoke procedure.
- `python3 scripts/format-rust-spacing.py --check` and `cargo fmt --all -- --check`; report existing drift rather than committing a repository-wide reformat.

Manual matrix:

| Scenario | Expected |
| --- | --- |
| Memory-only open | No managed folder, no sync/retry, no validation failure, "No index files are read or written" |
| Temporary/repository/custom on-demand | Store path/generations shown; status Searchable; sync becomes Synchronized |
| Fully resident | Preparing → Fully resident; validation counts shown |
| Pending changes | Status Pending; Synchronize now succeeds |
| Failed sync (read-only/full disk) | Access error, guidance, old generation retained, retry works after fix |
| Corrupt store | Corrupt category, rebuild guidance, no deletion |
| Incompatible store | Incompatible category, rebuild guidance |
| Lock conflict | Conflict category, retry guidance |
| Validation stamp warning | Index usable; warning only in validation details |
| Change storage | Prefilled, validated, explicit reopen; tabs preserved or warning; flush before switch |
| Close with pending changes | Flush; on failure Retry/Cancel; no false success |

Record exact commands, screenshots/terminal captures, and the final commit.

## 10. Commit sequence

1. **Core status kind and shared presentation helper** — `IndexStorageFailureKind`, status fields, mapping tests, `storage_view::describe`, presentation tests. No UI behavior change yet.
2. **TUI status and overlay** — fetch/display all fields, overlay current section and validation, custom path rules, synchronize/retry, help/docs, tests.
3. **Desktop status model and panels** — explicit fields, index panel and Diagnostics storage section, error rendering, tests.
4. **Desktop change-storage flow and synchronize action** — dedicated dialog, validation, controlled reopen, `UiAction`/backend task, tests.
5. **Desktop help/docs** — help paragraph and opening documentation.
6. **Final validation and report** — workspace tests, smoke matrix, task report, push. Keep integration branch for review; do not merge `master`.

## 11. Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Label drift between TUI and desktop | One shared `storage_view` helper with tests |
| Error misclassification changes recovery behavior | Map only presentation categories; never drive deletion or rebuild automatically |
| Change-storage flow loses tabs or unsaved changes | Flush first; preserve tabs where feasible; otherwise explicit confirmation listing the loss |
| Status polling adds overhead | Pure formatting, update only on change, no filesystem calls in status/render |
| Custom path resolves relative to an unexpected CWD | Canonicalize and display the resolved path; require an existing directory; mirror TUI rules in desktop |
| Validation fields remain dead presentation state | Require a render test that asserts each field is visible when set |
| Memory-only UI implies disk state | Explicit branch/hiding rules and tests for every storage row |
| Double-click/reentrant synchronize | Disable while running; generation-checked task; tests |

## 12. Deliverables and completion

1. Pushed commits on `integration/frontend-storage-parity`.
2. Updated TUI and desktop code, tests, and documentation.
3. A report with the parity matrix, screenshots/terminal captures, failure scenarios, exact commands, and remaining limitations.
4. `docs/ai/done/frontend-storage-parity.md` (or the task document location used by the supervisor) with an implementation report. Do not move it to `done/` until validated, committed, and pushed.
5. Keep the integration branch and worktree for the user; do not merge `master`.

Definition of done:

- Every capability in the parity matrix is present in TUI and desktop, with tests.
- Every `IndexStorageStatus` field is either rendered or intentionally documented as diagnostic-only.
- Synchronize/retry exists and is safe in both frontends.
- Change storage is a focused, validated, explicitly confirmed flow that flushes before switching and never silently loses work.
- Error categories and guidance match the table in Section 7.
- Memory-only shows no misleading disk state.
- Workspace tests, Clippy, CLI tests, and native desktop smoke pass; documentation is updated.

## Implementation report

Task identifier: `frontend-storage-parity`. Completed 2026-11-14 from base `79b8c02cc5` on `integration/frontend-storage-parity`. Integration worktree: `/home/simon/source/objekte-integration-frontend-storage-parity`.

The core now reports typed storage failure and validation-stamp warning kinds while retaining raw error text. The TUI and desktop share pure storage labels and recovery guidance. Both present effective location, residency, readiness, source reconciliation, synchronization, current and durable generations, validation counts, managed folder, and classified errors where applicable. Memory mode hides disk state. Both offer off-thread synchronize/retry controls. TUI storage changes flush before reopening and preserve the search/filter. The desktop has a focused storage dialog with a read-only source and an explicit warning that other tabs close; it keeps the current session if flushing or opening the replacement fails. Documentation and native evidence were added.

One supervisor implemented the task without subordinate branches. Implementation commits were `37cea91826` (typed status and shared presentation), `ea54cc661a` (TUI), and `c391d1119f` (desktop). Each was pushed immediately to `origin/integration/frontend-storage-parity`.

Validation: `cargo test --workspace --locked` passed with 995 tests and no failures (18 ignored). `cargo clippy --workspace --all-targets --locked` passed. `cargo test --locked -p objekte-desktop --example environment_validation` passed. CLI and desktop focused tests passed. The documented native desktop and GUI control procedures passed, including a failed access-path synchronization and retry; a TUI pseudo terminal smoke passed. `git diff --check` passed. `cargo fmt --all -- --check` and `python3 scripts/format-rust-spacing.py --check` still report repository-wide drift that is present in untouched `master` and the hybrid parent; no broad reformat was made.

No repository benchmark directly targets UI status rendering and storage-control actions, so no benchmark comparison was run. Native screenshots cover the four locations, full and on-demand residency, the focused dialog, invalid custom base, and access failure/retry. Corrupt/incompatible open guidance, conflict/resource labels, and validation-stamp warning presentation were verified in automated tests, without separate native screenshots for those fault modes. The detailed parity matrix, commands, and captures are in [the frontend storage parity report](../generated/frontend_storage_parity_report.md). No issue report was needed for an in-scope failure.

The original task specification was untracked in the original checkout; its complete contents were copied here before this report was appended. The user later requested merging this task into `integration/hybrid-index-storage` and removing merged worktrees. That supersedes the plan's request to retain this task's worktree; the original `master` checkout remains untouched. Cleanup and the final merge are reported separately after the report commit is pushed.

### Post-merge cleanup

On 2026-11-14, the user authorized merging into the parent integration branch and deleting merged worktrees. `integration/hybrid-index-storage` fast-forwarded from `79b8c02cc5` to the frontend report commit `0346ccfdda` and was pushed to `origin`. The completed frontend worktree and 17 clean sibling integration worktrees already merged into `master` were removed with `git worktree remove`. The hybrid integration worktree was kept for review; two unmerged index-construction task worktrees were retained. Four other directories in `source/` have no `.git` marker and are not registered worktrees, so they were not part of this cleanup. Available space rose from about 2.8 GB to 113 GB. The preserved untracked task source and its ignored temporary memory were removed after the done document had been pushed. `master` stayed at `da8fde6847`; its unrelated untracked files were not changed.
