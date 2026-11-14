# Custom-only saved-index opening needs the opening recipe

**Category:** feat  
**Originating task:** `mmap-index-v1`  
**Branch:** `integration/mmap-index-v1`  
**Locations:** `crates/desktop/src/controller/adapt.rs`, `crates/cli/src/index_open.rs`, `crates/core/src/index_storage/store.rs`

## Observed behavior

The desktop Open dialog and TUI can load a saved index when source paths are
still supplied, and the CLI accepts `--load-index PATH` with source arguments.
Opening a custom index directory without source paths does not yet reconstruct
the source selection from `frontend_metadata`.

## Expected behavior

The specification asks for a versioned opening recipe saved in
`frontend_metadata` so `obj --load-index /some/index-folder --fts ...` can
resolve schema, source registrations, and collection definitions without
recursive discovery.

## Impact

The saved index itself is complete and loads correctly when definitions are
supplied. Custom-only convenience opening remains unavailable.

## Possible approaches

1. Save a versioned JSON recipe in `SaveOptions.frontend_metadata` from each
   frontend.
2. Inspect the manifest, decode the recipe, resolve schema/source registrations,
   then call core Load with definitions and a configuration check.
3. Add CLI and desktop tests for a custom directory with no `--path`/`--repo`.
