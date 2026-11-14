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

## Resolution

Implemented for `mmap-index-v1`. Saves now include a versioned
`opening_recipe` object in manifest `frontend_metadata` containing source mode,
canonical root, selected folders, explicit schema, recursion, alphabet, and
auto-index. Native paths are hex-encoded and round-trip losslessly on Unix and
Windows. `obj --load-index <custom-folder> --fts ...` inspects the manifest,
decodes the recipe, rebuilds definitions/options, checks configuration identity,
and calls core Load without source arguments. `--index-alphabet` is now optional
so omission is distinct from an explicit ASCII choice; conflicting explicit
alphabet, schema, recursion, or inference options are rejected. CLI tests cover
custom-only query and storage-only opening, schema/inference/alphabet recipe
restoration, conflicting options, explicit source confirmation, and no-op
re-save. The desktop backend and Open spec have recipe-only load tests.

Reading the explicit schema file or repository configuration is allowed and is
the only source-side I/O during recipe resolution; source records are never
discovered or read by Load.
