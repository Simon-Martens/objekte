# Optional indexing, sorting, and streaming ledger

## Base and ownership

- Implementation base: `f374101b4839f28199a99677118ac96869203bb0` on `development/local-file-search-core`, fetched 2026-11-07. Its core tree is identical to `b70c7eef05`; the later commits are documentation and TUI changes.
- Integration worktree: `/home/simon/source/objekte-optional-indexing-sorting-streaming`, branch `integration/optional-indexing-sorting-streaming`.
- The original checkout had pre-existing deletions of `docs/development/corrective-results.md` and `implement.md` and an untracked task specification. TUI files also became modified during foundation work. None of those changes is owned by this task and none is discarded.
- Prior fixed 150-query and extended Musenalm recordings under `bench/results/musenalm/extended-b70c7eef05/` are applicable to the unchanged core and registered-schema workload. They are not a baseline for the new schema-free construction or the new sort/stream requests. The measured revision, release profile, 32 query workers, zero source and decoded caches, alphabets, five repetitions and corpus/schema fingerprints are recorded in that report.
- Installed compiler: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`.
- Existing experimental inference branch `experiment/schema-independent-field-indexing` at `02163b3271` is reference material only; work starts at the current development head.

## Shared contract checkpoint

The compiling foundation adds `CollectionDefinition::folder(name, relative_path, format)`. It is recursive, uses an empty `Schema`, has no sort columns, and does not read `.objekte` schemas. JSON uses `path:`-prefixed, extension-inclusive relative path identities; JSONL and registered readers use generated per-record identities. Low-level discovery keeps its existing nonrecursive default. Hidden files and symlinks remain skipped by default. The helper makes no inference decision: the opening option introduced by the inference worker must default to Disabled and must never silently report success for an inactive AutoIndex mode.

The following contracts are owned by the named workers. They describe behavior, not no-op public placeholders:

| Contract | Owner | Fixed rule |
| --- | --- | --- |
| Empty-array structural fact | Dynamic FILTER | One complete logical-record bitmap, independent of inference; shift batch-local slots at admission. |
| Inference mode/report/policy | Inference | Disabled by default; ReportOnly observes without postings; AutoIndex admits complete compatible postings. Initial repository-wide budget is 64 MiB, with at most 4,096 paths, 16 path steps, 64 distinct retained strings per provisional string path, 8 KiB retained distinct text per such path, and 64-byte admitted string values. Exceeding a bound declines the accelerator and marks a truncated report incomplete; it never changes accepted records. |
| Bound sort expression and comparison | Sorting | One normalized query-path/reducer form, explicit exceptional placements, kind rank Bool < Number < Date < Datetime < Unixtime < Id < String and stable collection/ID tie break. Stored and computed paths use the same comparison. |
| Streaming sink and cancellation | Streaming | Synchronous coordinator-thread callbacks with Continue/Cancel; external atomic cancellation token; distinct Complete/Cancelled/execution/sink outcomes; bounded page/event bytes. Cache publication happens only at successful completion. |
| Cache identity | Sorting and streaming | Verified membership is independent of page delivery; complete ordered results include normalized sort and generation; provisional pages never become complete cache entries. |
| Shared integration and snapshots | Supervisor | Merge structural and inference build hooks, reconcile storage versions, ranking and stream hooks, and preserve current source-access rules. |

Default source records remain object-valued under current reader validation. Arrays stay homogeneous per the current model. Snapshot keys contain the identity policy already; workers must extend compatibility for any newly persisted index or policy. Request structures may grow with documented defaults or explicit workspace-caller migrations. Binding, clock and regex preparation must occur once per execution. Existing return modes and source counters remain authoritative. Resource exhaustion must return an error, not a partial success.

## Tasks

| Task | Owner | State | Notes |
| --- | --- | --- | --- |
| TASK-00 | Supervisor | complete | Base and prior evidence pinned above. |
| TASK-01 | Supervisor | complete | Folder definition, path identity, example and compiling checkpoint. |
| TASK-02 | Dynamic FILTER worker | not started | Structural bitmap and conservative gram restriction. |
| TASK-03/04 | Inference worker | not started | Observation, policy, postings and restore. |
| TASK-05/06 | Sorting worker | not started | Computed and composite sorting. |
| TASK-07 | Streaming worker | not started | Delivery, cancellation and NDJSON. |
| TASK-08 | Supervisor | not started | Independent branch review. |
| TASK-09 | Supervisor | not started | Four-way integration and cross-feature validation. |
| TASK-10 | Supervisor | not started | Final measurements. |
| TASK-11 | Supervisor | not started | Development promotion. |

## Foundation validation

`cargo test --workspace --all-targets` and `cargo clippy --workspace --all-targets` passed on the foundation change. The recursive schema-free fixture found both `a/item.json` and `b/item.json`, returned `path:a/item.json` for its FTS query, and confirmed no typed posting or sort columns. JSONL received distinct generated IDs. The runnable example returned one Berlin match from `data/places` with explicit folder arguments. The repository-wide spacing check still reports pre-existing rustfmt drift in files outside this change, including untouched parts of `build.rs`; the new example and test are formatted. Clippy exited zero with advisory warnings in existing code. `git diff --check` passed.
