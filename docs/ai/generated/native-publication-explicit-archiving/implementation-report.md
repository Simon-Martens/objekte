# Native publication and explicit archiving: implementation report

Task: `native-publication-explicit-archiving`

Integration branch: `integration/native-publication-explicit-archiving`

Reviewed base: `dc371a42e666ef6c4d3c0519d6653326d0ad6d56`

Implementation revision for raw benchmark evidence:
`cd9b8275cd574df64c2889f2fe74565effbe656b`

## Outcome

Native indexes now publish and answer queries without archive serialization.
`Index::archive()` and `Repository::archive()` explicitly convert eligible
dictionaries and metadata, validate complete private replacements, and install
them atomically. Queries do not acquire the transition gate and continue on a
pinned prior publication while conversion runs.

No startup, frontend, mutation, compaction, or helper path calls archive
implicitly.

## Public API

Added:

- `Index::archive()`
- `Index::archive_with_options(ArchiveOptions)`
- `Index::archive_status()`
- `Repository::archive()`
- `Repository::archive_with_options(ArchiveOptions)`
- `Repository::archive_status()`
- `ArchiveOptions`, with a nonzero checked worker limit and a one-worker
  default
- `ArchiveReport`, including revision, instance ID, representation generation,
  component counts, byte/capacity totals, transition/preparation/installation
  timing, serializer and validator worker sums, worker limit, and observed peak
  active jobs
- `ArchiveStatus` and `ArchiveRepresentation::{Native, Mixed, Archived}`
- `search_ready` on `InitialBuildStatus` and `IndexStatus`; equivalent
  `IndexView::search_ready()`

The documented example is
`crates/core/examples/explicit_archive.rs`. It demonstrates build, search,
archive, search, mutate, and archive again.

## Storage and serialization

- Posting builders use one `BTreeMap<StoredKey, u32>` plus one canonical
  `Vec<RoaringBitmap>` from construction onward.
- Bitmap slots are insertion-assigned and survive publication and archiving.
- Native map and payload owners are immutable and shared by `Arc`.
- Explicit archive serializes the borrowed map and reverse directory directly.
  It does not create an ordinal tree, entry vector, sorted vector, or second
  native map.
- File/record row owners, file lookup hash maps, record-ID lookup hash maps,
  catalog metadata rows, catalog path hash/B-tree maps, and ownership maps have
  native and checked-archive backings.
- Raw/fallback/custom text, sorting, posting bitmaps, liveness, and relation
  structures retain shared native ownership. The report exposes their residual
  byte categories instead of claiming they are durable archives.

## Mutation and compaction

Replacement publications are always assembled from the current root. A
dedicated `RwLock<()>` transition gate is held shared by publishing writers and
exclusively by archive preparation/installation. New or rebuilt segments are
native. Unaffected archived components retain their owners. A later explicit
archive converts only native components.

Source-free compaction uses representation-independent row and dictionary
readers and publishes native compacted segments. Deletion remains a liveness
change and does not rebuild every posting dictionary.

## T01-T40 evidence map

| Test | Executed evidence |
| --- | --- |
| T01 | `rkyv_storage_diagnostics_do_not_populate_caches_or_clone_payloads`; startup counters assert zero checked validations and zero archive bytes |
| T02 | Existing progressive lifecycle tests plus `explicit_archive_rejects_incomplete_build_without_waiting_for_workers`; native partial searches remain marked incomplete |
| T03 | `native_startup_and_empty_archive_have_explicit_boundaries` and existing zero-record lifecycle tests |
| T04 | Existing blocking-build and `wait_complete` tests; startup timing paths no longer call archive |
| T05 | Full core suite and 150-case native query run |
| T06 | `explicit_archive_is_blocking_atomic_and_supports_mixed_lifecycle`; fresh view reports archived representation and uses archived readers |
| T07 | Native/archive result digest files are byte-identical across 300 alphabet/case combinations |
| T08 | `rkyv_forward_and_descending_bounds_match_native_oracle` and descriptor reverse traversal tests |
| T09 | `publishing_preserves_insertion_assigned_bitmap_slots` and archived reverse-directory tests |
| T10 | `validation_rejects_a_nonempty_unreachable_payload` and empty-posting tests |
| T11 | Existing first-batch adoption counters/tests and native map-plus-payload transfer |
| T12 | `allocation-summary.tsv`; source adapters borrow maps, and tests assert `temporary_ordinal_trees == 0` |
| T13 | `validated_archive_owns_and_borrows_checked_bytes`; borrowed sources are dropped before archive access |
| T14 | `rkyv_semantic_validation_rejects_tree_reverse_directory_disagreement`, malformed archive tests, row/lookup association validation |
| T15 | Pre-archive pinned query in `explicit_archive_is_blocking_atomic_and_supports_mixed_lifecycle` completes against native storage |
| T16 | `queries_and_quiesce_coordinate_with_a_paused_archive`; query completes while archive is paused |
| T17 | `rkyv_archive_finalization_failure_publishes_nothing`; revision, generation, query result, and publication remain unchanged |
| T18 | Same lifecycle test performs repeated archive and checks zero conversion, unchanged generation, owners, serialized bytes, and capacity |
| T19 | Weak-owner assertion in the lifecycle archive test releases the pre-archive segment owner after the pinned view drops |
| T20 | Pointer-owner assertions cover posting bitmaps, segment text, base text, and base sorting |
| T21 | Revision, file handle, and query identities are asserted across archive |
| T22 | `concurrent_archive_calls_serialize_without_duplicate_conversion` |
| T23 | Incomplete gated-build archive test returns promptly; shutdown/quiesce test returns `Cancelled` |
| T24 | Paused archive plus concurrent quiesce test drains without deadlock and prevents late installation |
| T25 | Mixed add, query, second archive, delete, compaction, and final archive in the lifecycle test |
| T26 | Existing replacement lifecycle matrix plus archived compaction coverage |
| T27 | Mixed delete and complement/query path with unaffected archived dictionaries |
| T28 | Existing compatible-move tests and catalog path tests |
| T29 | Transition-gate implementation; existing concurrent publication retry tests |
| T30 | Existing smallest-segment and append-only mutation tests |
| T31 | `rkyv_source_free_compaction_removes_tombstones_without_source_opens` plus archived lifecycle compaction |
| T32 | Lifecycle test rearchives after mutation/compaction; only new native owners convert |
| T33 | `rkyv_catalog_preserves_non_utf8_exact_paths_and_distinct_query_keys` plus archived catalog lookup |
| T34 | Existing catalog rebase/ownership tests |
| T35 | Archive-row validation avoids compatibility caches; `metadata_cache_initializations == 0` and row-cache bytes remain zero |
| T36 | Existing repeated lifecycle tests and weak-owner release test |
| T37 | Repository archive/no-op/failure behavior covered by defaults and focused repository tests |
| T38 | Existing no-write test; archive code has no file-source or cache path |
| T39 | `StorageUsage`, `ArchiveStatus`, and `ArchiveReport` expose native/archived counts, shared payload bytes, archive capacity/slack, and pinned owners |
| T40 | Workspace all-target check covers CLI, TUI, and desktop; no frontend call sites invoke archive |

## Validation performed

```text
cargo test -p objekte-search-core --lib
  result: 592 passed; 17 ignored; 0 failed

cargo check --workspace --all-targets
  result: passed

cargo run --release -p objekte-search-core --example explicit_archive -- <writable-root>
  result: native/archived/mixed/final query counts 1/1/2/2

python/`jq` comparison of the 300 native and archived query measurements
  result: result digests identical; 0 semantic mismatches
```

Additional focused tests run during implementation include catalog shard tests,
the mixed lifecycle archive test, the paused-query/quiesce test, storage-cache
diagnostics, weak-owner release, and concurrent archive serialization.

Final workspace tests, compaction integration tests, clippy/check, and
detailed-profiling checks are listed in the task completion record.

## Performance and memory

The smaller user-authorized benchmark used one repetition over the unchanged
150-case Musenalm workload per alphabet:

| Alphabet | Native total | Archived total | Archived/native |
| --- | ---: | ---: | ---: |
| ASCII | 601.567 ms | 597.728 ms | 0.99362x |
| Unicode | 601.152 ms | 603.509 ms | 1.00392x |
| Both | 1202.719 ms | 1201.237 ms | 0.99877x |

The first archived query is cold: Q001 was 53.438 ms ASCII and 74.233 ms
Unicode, versus 7.727/7.196 ms native. Warm Q001 medians were 0.0216/0.0203 ms
native and 0.0238/0.0205 ms archived. Q119 was 1.0934x native for ASCII and
1.1526x for Unicode in the one-repetition cold run.

The progressive path was also run over all 150 cases per alphabet. Its result
digests were identical. Aggregate progressive time was 0.97307x native for
ASCII, 1.05243x for Unicode, and 1.01185x over both.

Native startup was 745.907 ms ASCII and 805.718 ms Unicode with zero archive
bytes. End/peak RSS was 690.1/753.3 MiB and 1366.6/1384.3 MiB.

Explicit archive at one worker took 175.230 ms ASCII and 149.572 ms Unicode.
It generated 149,111,376 serialized bytes with 233,891,392 allocated bytes
and 84,780,016 bytes of capacity slack. Eight workers reached peaks of seven
active conversion jobs; thirty-two workers did not exceed that seven-job
scheduler window for this segment topology. The larger worker settings did not
produce a reliable speedup and showed run-to-run contention.

Archived end RSS was 908.3 MiB ASCII and 1667.5 MiB Unicode in the final
one-worker query run. This includes archive buffers, retained shared native
payloads, and allocator-retained pages; it is not a claim of immediate owner
release.

## Limitations

- The benchmark is smaller than the seven-sample/five-sample protocol because
  the user explicitly authorized a smaller run.
- Native and archived query timing used one repetition per case; cold and warm
  measurements are reported separately.
- Relation payloads remain native in this task because they are not yet a
  durable archive category.
- Archived query cold-start compatibility row materialization remains a visible
  cost; warm reusable results do not show the same gap.
