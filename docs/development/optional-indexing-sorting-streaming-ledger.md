# Optional indexing, sorting, and streaming ledger

## Base and ownership

- Implementation base: `f374101b4839f28199a99677118ac96869203bb0` on `development/local-file-search-core`, fetched 2026-11-07. Its core tree is identical to `b70c7eef05`; the later commits are documentation and TUI changes.
- Integration worktree: `/home/simon/source/objekte-optional-indexing-sorting-streaming`, branch `integration/optional-indexing-sorting-streaming`.
- The original checkout had pre-existing deletions of `docs/development/corrective-results.md` and `implement.md` and an untracked task specification. TUI files also became modified during foundation work. None of those changes is owned by this task and none is discarded.
- Prior fixed 150-query and extended Musenalm recordings under `bench/results/musenalm/extended-b70c7eef05/` are applicable to the unchanged core and registered-schema workload: `git diff b70c7eef05..f374101b48 -- crates/core` is empty. The extended report measured `b70c7eef05` with corpus fingerprint `9f93a4ba7fb9249c9c4eb04e03ed911435a6b0493f15b087a5447add32456aa9`, schema digest `f94dd058ab3d27be17b5e475089b0b83580d6e960d566bd54032157298da0eed`, release profile, 32 query workers, zero source and decoded caches, ASCII/Unicode, verified cache off/64 MiB warm, and five repetitions. The earlier fixed 150 run used the same corpus, compiler and profiles but a different schema digest because it did not load the opt-in relation annotations. Neither is a baseline for new schema-free construction or new sort/stream requests. Final comparison must revalidate these fingerprints.
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
| TASK-02 | Dynamic FILTER worker | accepted | Pushed clean head `0fbf439aa9` on `feature/dynamic-filter-bigrams`; structural bitmap, sound gram restriction and focused evidence reviewed. Merge awaits TASK-08 gate. |
| TASK-03/04 | Inference worker | accepted | Clean pushed head `a34154091a`; four review findings corrected, focused/core/workspace checks passed, and final read-only recheck found no blocker. Merge awaits TASK-08 gate. |
| TASK-05/06 | Sorting worker | accepted | Clean pushed head `4e3a6f4325`; three review gaps corrected, workspace tests/Clippy passed, final read-only review found no blocker. Merge awaits TASK-08 gate. |
| TASK-07 | Streaming worker | correction in progress | `48adf895ed` passed tests/Clippy and NDJSON smoke; independent review found three sequence/resource gaps below. |
| TASK-08 | Supervisor | in progress | Dynamic, inference and sorting accepted; streaming requires focused corrections and recheck before merge. |
| TASK-09 | Supervisor | not started | Four-way integration and cross-feature validation. |
| TASK-10 | Supervisor | not started | Final measurements. |
| TASK-11 | Supervisor | not started | Development promotion. |

## Foundation validation

`cargo test --workspace --all-targets` and `cargo clippy --workspace --all-targets` passed on the foundation change. The recursive schema-free fixture found both `a/item.json` and `b/item.json`, returned `path:a/item.json` for its FTS query, and confirmed no typed posting or sort columns. JSONL received distinct generated IDs. The runnable example returned one Berlin match from `data/places` with explicit folder arguments. The repository-wide spacing check still reports pre-existing rustfmt drift in files outside this change, including untouched parts of `build.rs`; the new example and test are formatted. Clippy exited zero with advisory warnings in existing code. `git diff --check` passed.

## Interface and integration notes

- All first three workers branched from immutable checkpoint `57650298e9` in separate worktrees. Streaming starts from that same commit when a writing slot is free.
- Sorting will retain legacy `SortField.field` and add a serde-defaulted typed `expression: Option<SortExpression>`. Inline reducers and paths normalize to this expression. Streaming must consume the common bound key/rank path rather than assume resident columns.
- Dynamic FILTER adds a persisted collection-level empty-array bitmap and locally bumps `SNAPSHOT_VERSION` 9→10. Inference and sorting snapshot changes must be reconciled after merging, not by independently assuming that version 10 is final.
- Selector review at the checkpoint found that `walk_path`/`walk_query_steps` can report a present empty selection only when an empty array is traversed implicitly or via `[*]`; direct out-of-range `[n]` is missing. The final dynamic path must still retain the residual, honor reader coverage, and avoid skipping fallible operations.

## TASK-02 review

Accepted branch commits are `7007f8eea1` (structural E construction/admission/snapshot), `6650aa15b4` (Unknown string candidate path, plans and independent fixtures), and `0fbf439aa9` (proof and validation report). The branch is clean and pushed. Its workspace test and Clippy commands passed, with 79 advisory warnings matching foundation. A previous positional-plan assertion was narrowly updated because the newly proven positive literal path now uses RawBigram with a residual. Read-only supervisor review checked the selector proof, nonfallible matching gate, reader/raw coverage checks, logical-only E, batch slot shift, v10 snapshot rejection and corruption checks, both alphabets, custom-reader coverage, metadata OR fallback, cache parity and structural parse-reduction counters. No blocking finding remains; final cross-feature integration will recheck the combined build and snapshot layout.

## TASK-03/04 independent review

The first inference head `fe282ef41b` was pushed and passed full workspace tests and Clippy. A read-only reviewer found three verified gaps, so it remains unaccepted pending a focused correction: (1) the batch observation cap derived from `2 × build_workers` can change admission and reports for identical records; (2) nested arrays such as `{"x":[[1]]}` underreport array depth; (3) inferred exact `:exists`/`:null` paths are unreachable because the candidate gate requires `Unknown` while the binder assigns `Bool`. An exact-proof fix must also reject explicit array selectors where their semantics differ from an implicit path. The reviewer withdrew an initial terminal-array concern after checking that nonterminal recursive visits do not alter the scalar/array shape flags. No ordinary equality/range candidate false negative was found in the inspected code. The correction worker owns fixes and deterministic fixtures; integration awaits its validated head.

The correction `c8362668f9` fixed those three findings, passed 12 focused inference tests, the workspace suite and Clippy, and was pushed. A second independent read-only pass found that `ScalarKey::from_value` still cloned an arbitrarily long string before the optional-index length guard, even after the path was dropped. The 64 MiB policy is an inference-state estimate, not an RSS ceiling; the correction fixes per-batch caps, limits in-flight jobs and shares the coordinator allowance across simultaneously retained collections.

The final inference follow-up `a34154091a` skips owned key construction for overlong, dropped and ReportOnly values. A test-only key-build counter covers 1 MiB nonblank text, overlong blank text, dropped paths and ordinary short values. Focused and core all-target tests and Clippy passed. The final independent read-only review accepted this head and found no candidate-superset blocker. The estimate caveat above remains in the public inference documentation.

## TASK-05/06 independent review

The pushed sorting head `10f41b8a50` implements computed local/related reductions, total scalar order, bounded top-k, resident composite orders and verified-cache reranking. Workspace tests and Clippy passed; focused inline-sort and comparator tests passed. A read-only reviewer verified the transformed-null and one-bind fixes, but found three gaps: warm cached computed sorting materializes all member ranges in an uncharged grouping map; cross-collection sort compatibility omits registered binding/version identity; and composite registration rejects typed simple expressions that lookup normalizes. Its local snapshot version 10 must be reconciled with dynamic/inference versions.

The correction `4e3a6f4325` precharges cached membership and a flat grouped-read vector, checks live winner/target bytes while reranking, compares registered transform name/version across collections, and normalizes typed simple composite definitions at registration. Workspace tests and Clippy passed. Final independent read-only review accepted this head with no remaining concrete correctness blocker. Its 128 MiB budget is a retained-state estimate, not a hard RSS ceiling; allocator overhead and a diagnostic touched-file set are not charged.

## TASK-07 independent review

The pushed streaming head `48adf895ed` provides a synchronous sink/cancellation API, ordered rows, replacement pages, warm cached membership streaming and versioned Musenalm NDJSON. Workspace tests and Clippy passed; the CLI smoke produced parseable `ordered_rows` then `complete` records with diagnostics on stderr. A read-only reviewer found three gaps: `final_sequence` can point to a progress event rather than the last page; target-prefetch limit failures can be misclassified as execution errors; and coalesced source reads can allocate a larger window than a small stream byte budget. The owning branch is correcting these. Computed-sort and computed warm-cache delivery remain mandatory integration work.

## TASK-09 merge map

All feature branches forked from `57650298e9`. The anticipated shared-file conflicts are semantic: `build.rs` must retain both the structural empty-array bitmap and inference observation on the already parsed logical value; `repository.rs` must hold both states and expose streaming; `executor.rs` must keep the dynamic RawBigram/E path and inferred typed postings with their proof guards; `storage.rs` must combine structural, inference and composite restore checks and use a final snapshot version above all worker-local versions (12 planned). Sorting's computed `RankKey`/`BoundSort`/composite cursor is the ranking base in `run.rs`; streaming's optional `Delivery`, early cursor events, cancellation, resource accounting and one completion/cache decision must be grafted into that common execution. Computed warm membership requires delivery during bounded reranking. The streaming CLI help text must be updated for unregistered sorts. Focused compile/tests follow each merge, then Section 9 interaction checks.
