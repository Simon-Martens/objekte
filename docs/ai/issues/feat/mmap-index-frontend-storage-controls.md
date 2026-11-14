# Desktop and TUI lack complete package save/load controls

**Category:** feat  
**Originating task:** `mmap-index-v1`  
**Branch:** `integration/mmap-index-v1`  
**Locations:** `crates/desktop/src/ui/open_dialog.rs`, `crates/desktop/src/ui/panels.rs`, `crates/cli/src/tui/mod.rs`

## Observed behavior

The CLI exposes `--load-index`, `--save-index`, and `--check-index`, and the
desktop backend understands those arguments. The TUI has an F6 index-storage
overlay that reports origin, location, mapped/resident bytes, publication,
changes, freshness, save progress, and can save or check sources. The desktop
has the existing Index panel with source checking and maintenance actions.

## Expected behavior

The specification asks for a complete Open-dialog index-source section, a
desktop Save dialog with progress and cancellation, a TUI storage overlay with
Load and custom-location controls, and shared location preferences.

## Impact

Users can build, save, load, and check through the CLI, and the TUI can save and
check a running index. The desktop Open dialog cannot yet select Build vs Load,
a storage location, or a custom directory, and neither GUI exposes a Load action
that replaces the current session.

## Why not resolved here

The desktop open dialog and controller have generation-ownership rules for
open/load/save completion that need a coordinated UI, controller, and worker
change. The TUI load action needs a replacement-session protocol and state
invalidation for catalogues, results, detail locators, relations, and
completion state.

## Possible approaches

1. Reuse `objekte_cli::index_location` for the Open and Save dialogs.
2. Add controller storage tasks keyed by operation generation, mirroring the
   existing index-task ownership checks.
3. In the TUI, load into a fresh `IndexedRepository` and install it only after
   the replacement view is valid.
