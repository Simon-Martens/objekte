# Catalog-led lazy storage implementation report

Date: 2026-11-17

Task: `catalog-led-lazy-storage`

Integration branch: `integration/catalog-led-lazy-storage`

Implementation revision covered by this report:

`3f7140779f`

Merged base: `15f45465ed0b7455577fce5431de50dfeea1ee16`

## Verdicts

| Area | Verdict | Evidence |
| --- | --- | --- |
| Correctness | Pass for the exercised round trips | Native and reopened search, mutation, compaction, relation, FTS, sort and empty-index tests pass. |
| Eagerness/laziness | Partial | Eager open performs no selected payload reads and `EXPLAIN` is payload-independent. FTS, posting values/state, and configured sort columns use independent lazy units. |
| Durability | Partial | Dependencies are synchronized before `CURRENT`, uncertain root sync is classified, and interrupted staging is pruned. Deterministic fault injection and process-abort barriers are not implemented. |
| Lifecycle/mutations | Pass for the exercised cases | Add, delete, replace, move, multi-generation save and source-free compaction pass. The required 128-step model matrix is not implemented. |
| Native construction | Pass structurally | Native build and memory-only archive create no storage files and serialize no storage payload. No new release construction comparison was run after this repair. |
| Warm-query overhead | Pass in the bounded smoke benchmark | Stored FTS executes through the indexed planner; repeated Q001 and Q119 produced no additional index reads. |
| Cold-query latency | Partial | Selected FTS leaves load lazily and coalesce. Other posting leaves remain field-coarse and sort payloads remain collection-coarse. |
| Memory | Partial | Cache trim/reload and shared decoded FTS leaves are covered. Full ownership/pinning, eviction and allocator accounting are incomplete. |
| Disk amplification | Partial | Deletion-only saves leave payload packs untouched and fresh-store export is self-contained. Object-size and fragmentation matrices are incomplete. |

## Implemented repairs

- Stored FTS dictionaries are separated from normal/skip Roaring leaves. A missing
  gram returns an empty candidate set without reading sibling leaves.
- Stored posting dictionaries contain dense key ordinals and independently addressed
  value, length and structural leaves. An equality filter loads one selected posting
  leaf, and concurrent first access shares both the dictionary and leaf load.
- Selected posting/FTS/sort/relation I/O failures propagate through query execution
  rather than being interpreted as an absent index.
- `EXPLAIN` remains metadata-only after every selected lazy payload is corrupted.
- Unsorted queries and sort binding do not force lazy sort payload loads.
- Reopened stores retain all referenced immutable packs across resaves and exports.
- Same-store deletion-only saves avoid reading or rewriting unchanged payload packs.
- Destination bindings are tracked per explicit storage directory, so exporting a
  copy does not discard the original store binding.
- Staging names are create-new and retry-safe; offline prune removes only recognized
  interrupted staging files and leaves unrelated files untouched.
- Existing writer locks and positioned reads use portable standard-library APIs.
- Relations have a checked lazy payload path and selected relation queries load it.
- Eager open remains source-free and payload-free, while selected reads are bounded
  by the configured concurrent-read permit.
- Empty completed snapshots, source-root incompatibility, corrupt-current pruning,
  native no-storage behavior and in-memory archive behavior have explicit tests.

## Validation

Commands completed in the integration worktree:

```sh
cargo check --workspace --all-targets
cargo test -p objekte-search-core
cargo test --workspace
cargo test --release -p objekte-search-core --test storage_roundtrip
```

Results:

- Workspace all-target check: passed.
- Core: 601 unit tests passed, 17 ignored; all core integration and doc tests passed.
- Workspace: all CLI, desktop, core, integration, process and doc tests passed.
- Release storage round trips: 27 passed.

The bounded Musenalm smoke evidence is recorded in
`storage-smoke-benchmark.md`. It is not a substitute for the complete 150-query,
worker, mutation, cache-budget and memory matrices required by the specification.

## Remaining acceptance work

The prototype is not ready to be represented as full specification completion.
`test-matrix.tsv` currently records 38 passing, 17 partial and 25 pending rows.
The principal remaining gaps are:

- replace the JSON snapshot body with independently archived eager catalog/map
  schemas and persisted shard reuse;
- split segment-local relation contributions into independently loaded payloads;
- add the complete lease/pinning, permanent-failure, cancellation and file-handle
  cache model;
- add deterministic `StorageIo` fault injection, process-abort transaction barriers
  and sync trace tests;
- run the required full construction, save, open, query, mutation, compaction,
  cache-budget, memory and object-size matrices from raw samples.

These are implementation/validation gaps, not failures hidden by the passing test
suite.
