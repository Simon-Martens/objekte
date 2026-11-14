# File content generations are not yet separate from mutation versions

**Category:** feat  
**Originating task:** `mmap-index-v1`  
**Branch:** `integration/mmap-index-v1`  
**Locations:** `crates/core/src/inventory.rs`, `crates/core/src/lifecycle_catalog.rs`, `crates/core/src/index_storage/native_pack.rs`

## Observed behavior

The current inventory exposes `FileVersion`, which advances for accepted file
mutations. Saved catalogue rows write that value into both `file_version` and
`content_generation`. The native state preserves the existing published view.

## Expected behavior

The specification asks for a separate content generation that advances when the
extracted contribution changes, remains stable for a proven content-preserving
move, and is used to retire old run contributions.

## Impact

Persistence and reload are correct for the existing lifecycle model, but a
future streaming compaction cannot use the stored value to distinguish a
metadata-only move from a content change.

## Why not resolved here

Adding content generations requires changing file mutation, move, verified
update, compaction, and catalogue persistence together. The existing lifecycle
already uses `FileVersion` for conflict detection and does not expose an
independent contribution token.

## Possible approaches

1. Add `content_generation` to `FileEntry`/the lifecycle catalog and advance it
   only in replacement and path-dependent mutation paths.
2. Preserve it through compaction and save it in `FileRowInput` independently
   of `file_version`.
3. Add lifecycle tests for move, delete/re-add, and repeated large-file
   replacement.
