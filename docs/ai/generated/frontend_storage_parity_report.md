# Frontend storage parity validation

Task: `frontend-storage-parity`
Base: `79b8c02cc5`
Implementation: `37cea91826`, `ea54cc661a`, `c391d1119f`
Branch: `integration/frontend-storage-parity`
Date: 2026-11-14

## Parity matrix

| Capability | TUI | Desktop | Verification |
| --- | --- | --- | --- |
| Memory, temporary, repository, custom location | Storage overlay | Open and focused Change storage dialog | Unit tests; native screens for all four |
| On-demand and full residency | Storage overlay | Open and Change storage dialog | Unit tests; native temporary on-demand and repository full screens |
| Readiness, reconciliation, sync, generations, validation, path | Current section and bounded status line | Index and Diagnostics panels | Shared presentation tests, controller tests, native screens |
| Memory hides disk details and sync action | Yes | Yes | Unit tests; native memory screen |
| Classified failure and guidance; raw diagnostic | Overlay and error line | Index panel and Diagnostics | Typed mapping tests; corrupt managed-open test; native access-failure screen |
| Explicit sync and retry without closing | Background worker | Controller worker | Controller retry test; native access failure and retry |
| Change setting with current work protected | Flush before reopen; preserve search and filter | Focused dialog; flush before replacement; warn that other tabs close | Reopen and flush-failure tests; native dialog |

The shared `objekte_cli::storage_view` helper generates labels and recovery guidance. The core keeps the original error strings and adds typed failure categories. The CLI NDJSON status adds `last_error_kind` and `validation_stamp_error_kind` without a protocol version change.

## Checks

Executed from `/home/simon/source/objekte-integration-frontend-storage-parity` with the repository's locked dependencies:

| Command | Result |
| --- | --- |
| `cargo test --workspace --locked` | Passed: 995 tests, 0 failures, 18 ignored |
| `cargo clippy --workspace --all-targets --locked` | Passed; existing warnings remain |
| `cargo test --locked -p objekte-desktop --example environment_validation` | Passed: 1 test |
| `cargo test -p objekte-cli -p objekte-desktop --locked --quiet` | Passed during implementation |
| `git diff --check` | Passed |
| `cargo fmt --all -- --check` | Fails due to repository-wide formatting drift also present in untouched `master` and the hybrid parent branch |
| `python3 scripts/format-rust-spacing.py --check` | Fails due to repository-wide spacing drift also present before this task |

Native GUI validation used `bash scripts/gui/run_desktop.sh prepare` and `bash scripts/gui/run_desktop.sh cleanup`. The application was opened against a small JSON fixture under ignored `target/gui-validation/frontend-fixture`, and the Index panel was inspected in memory, temporary on-demand, repository full-residency, and custom on-demand modes. The focused storage dialog showed the active source as read only. An invalid custom folder was rejected. An obstructed store lock produced an access failure while the current generation remained searchable and the durable generation stayed at 3; after removing the obstruction, **Retry synchronization** made generation 4 durable.

The native control validation ran:

```text
bash scripts/gui/run_gui_validation.sh prepare
bash scripts/gui/run_gui_validation.sh exercise 400 132 34 173 76 214 500 450 180 214
bash scripts/gui/run_gui_validation.sh cleanup
```

The saved fixture was `{"text":"Objekte ÄÖÜ äöü ß 123","checked":true,"action_count":1}`. The headless session was stopped afterward. The TUI process smoke used the built `obj --path target/gui-validation/frontend-fixture --index-storage repository --tui` under a pseudo terminal, opened the Index storage action with Ctrl+K, navigated it, and closed with Ctrl+Q; the process exited successfully.

## Evidence

- [Memory Index panel](frontend-storage-parity/storage-memory-index-panel.png)
- [Temporary Index panel](frontend-storage-parity/storage-temp-index-lower.png) and [Diagnostics](frontend-storage-parity/storage-temp-diagnostics-storage.png)
- [Repository full residency](frontend-storage-parity/storage-repo-full-index-panel.png)
- [Custom Index panel](frontend-storage-parity/storage-custom-index-panel.png)
- [Focused change dialog](frontend-storage-parity/storage-change-dialog.png) and [invalid custom base](frontend-storage-parity/storage-custom-invalid.png)
- [Failed synchronization](frontend-storage-parity/storage-custom-failed-retry.png) and [successful retry](frontend-storage-parity/storage-custom-retried.png)
- [TUI pseudo terminal capture](frontend-storage-parity/tui-overlay.ansi)
- [Native control fixture](frontend-storage-parity/native-control-fixture.json)

Corrupt and incompatible open guidance is covered by the shared presentation tests; corrupt on-disk manifest handling is also covered by the desktop backend test. Conflict, resource, and validation-stamp warning presentation is covered by typed and synthetic status tests. Separate native screenshots were not taken for these fault modes. The production GUI smoke covered normal storage choices, custom validation, access failure, retry, and the focused change flow. No repository benchmark directly targets these presentation and control changes, and no performance comparison was made.

The integration branch was not merged into `master`. The original checkout's other untracked documents were left untouched.
