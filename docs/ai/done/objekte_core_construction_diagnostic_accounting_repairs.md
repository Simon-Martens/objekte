# Assignment 2 follow-up — Construction benchmark and diagnostic accounting

**Reviewed master** `1811f49160e45e9e5a827cfa8b9bfca42cbf9a5f`  
**Task** `core-construction-diagnostic-accounting`  
**Branch** `integration/core-construction-diagnostic-accounting`  
**Sibling worktree** `/home/simon/source/objekte-integration-core-construction-diagnostic-accounting`

The pinned logical counts, live locator selection and deterministic ownership regression from Assignment 2 are implemented. The following work concerns benchmark measurements and observation overhead. It can land separately from Assignment 3 and from the post-merge module cleanup. Use the actual owner paths on the integrated base.

## 1. Sample resident memory after releasing the index

In the default branch of `bench_index_construction.rs`, the local `index: IndexedRepository` retains both an `Index` and an `Arc<Repository>`. The code drops `repo: IndexView` and samples `after_release_rss` while the owner is still in scope. The incremental benchmark explicitly drops its handles before this measurement. The construction time and the peak RSS were captured earlier. [Benchmark owner](https://github.com/Simon-Martens/objekte/blob/1811f49160e45e9e5a827cfa8b9bfca42cbf9a5f/crates/core/examples/bench_index_construction.rs#L145) · [Misordered measurement](https://github.com/Simon-Martens/objekte/blob/1811f49160e45e9e5a827cfa8b9bfca42cbf9a5f/crates/core/examples/bench_index_construction.rs#L281) · [Retained handles](https://github.com/Simon-Martens/objekte/blob/1811f49160e45e9e5a827cfa8b9bfca42cbf9a5f/crates/core/src/lifecycle_caller.rs#L22)

- Retain the owner throughout build completion, query validation and storage probes. Copy out only scalar/owned report values needed later. Immediately before reading `after_release_rss`, drop the view, then drop its owner. Use explicit drops beside the sampling point. Match the meaning of the existing incremental-mode field.
- Keep the existing field name, timer boundaries, `after_build_rss` and peak measurement. The field measures process RSS after releasing the benchmark’s handles. Do not force allocator trimming, sleep or assert that RSS must fall. An allocator may retain freed pages.

## 2. Aggregate the segment observations without an intermediate vector

`IndexView::index_observation` uses one pass over catalogue rows. Its segment loop still calls `segment_summaries()`, which collects every segment into a temporary vector before the loop starts. `bench_build_musenalm` then performs the same physical summary pass again for its per-segment output. [Observation consumer](https://github.com/Simon-Martens/objekte/blob/1811f49160e45e9e5a827cfa8b9bfca42cbf9a5f/crates/core/src/lifecycle/view.rs#L165) · [Vector producer](https://github.com/Simon-Martens/objekte/blob/1811f49160e45e9e5a827cfa8b9bfca42cbf9a5f/crates/core/src/repository.rs#L848) · [Benchmark consumer](https://github.com/Simon-Martens/objekte/blob/1811f49160e45e9e5a827cfa8b9bfca42cbf9a5f/crates/core/examples/bench_build_musenalm.rs#L139)

- Extract a crate-private iterator over summaries computed from the supplied pinned segment groups. Build each summary once using the existing accounting code. The iterator yields a small owned summary for each borrowed segment. It must take the groups from the specified view. Retain collection/segment iteration order.
- Consume this iterator directly in `index_observation`. Keep its per-collection accumulators. For the diagnostic caller that needs both the aggregate and individual segment IDs/counts, share the traversal. A concrete design is an observation method accepting a visitor over `&NativeSegmentSummary`, invoked as each summary is folded. Implement the aggregate-only operation using the same fold with an empty visitor. The benchmark can append the required `(id, records, files)` tuples in the visitor and sum `index_family_bytes` there. Use the observation’s existing gram and inventory totals. Retain an explicit vector only at call sites whose requested output is the complete segment listing.
- In the incremental construction benchmark, accumulate file count and byte count in the existing catalogue fingerprint loop; it currently walks the same rows three times. Preserve the row order and hash inputs. In the default branch, move the detailed inventory storage diagnostic behind `--storage-diagnostic`; reuse the normal observation’s inventory heap totals. [Three catalogue walks](https://github.com/Simon-Martens/objekte/blob/1811f49160e45e9e5a827cfa8b9bfca42cbf9a5f/crates/core/examples/bench_index_construction.rs#L439) · [Optional diagnostic](https://github.com/Simon-Martens/objekte/blob/1811f49160e45e9e5a827cfa8b9bfca42cbf9a5f/crates/core/examples/bench_index_construction.rs#L211)

## 3. Label the build-time bitmap measurement precisely

`CollectionIndexObservation::structural_bitmap_bytes` is calculated solely from `collection.empty_array_records.serialized_size()`. Construction installs that bitmap from the completed build metadata. This is one serialized bitmap size, and newly published segments are absent from that value. Name the field `build_empty_array_bitmap_serialized_bytes` and document that provenance. Migrate any maintained consumer or serialized observation fixture that reads this field. Keep independent benchmark measurements whose own definition and scope differ. [Initialization](https://github.com/Simon-Martens/objekte/blob/1811f49160e45e9e5a827cfa8b9bfca42cbf9a5f/crates/core/src/lifecycle/view.rs#L145) · [Build metadata installation](https://github.com/Simon-Martens/objekte/blob/1811f49160e45e9e5a827cfa8b9bfca42cbf9a5f/crates/core/src/construction/repository_state.rs#L480)

## Acceptance

1. Run the existing pinned-count, shared-file, locator-deletion and ownership regressions. Check that per-segment observations agree with the normal aggregate after an add and delete with an older view retained. Exercise several segments and two collections. Compare the new build-time bitmap measurement to the retained build metadata even after publication of another segment.
2. Run `cargo fmt --all -- --check`, `cargo check --locked --workspace --all-targets` and `cargo test --locked -p objekte-search-core --all-features`. Smoke-run the default and `--incremental` construction benchmark on the same small fixture, enabling detailed storage on the default run. Compare all unchanged counts and fingerprints; verify by source order that both RSS samples follow release of the benchmark's handles. A full corpus timing suite is unnecessary.
3. Use a separate commit for the RSS correction and for the diagnostic aggregation. Record the tested SHA and the two benchmark command lines in the implementation report. Follow `docs/ai/README.md` for delivery. Leave the merge to the owner.

## Implementation report

- Task identifier: `core-construction-diagnostic-accounting`
- Completion date: 2026-11-18
- Base commit: `c982344f87c9ff01d1316bba4b9b3d7c5bde53e7`
- Integration branch: `integration/core-construction-diagnostic-accounting`
- Integration worktree: `/home/simon/source/objekte-integration-core-construction-diagnostic-accounting`
- RSS correction commit: `4f4917aedb`
- Diagnostic aggregation commit: `fa193db9e2`
- Tested implementation head: `fa193db9e2`

The default construction benchmark now keeps the completed `Index` ownership
chain alive through query validation and storage probes. It explicitly drops
the pinned view and then the `IndexedRepository` immediately before sampling
`after_release_rss`. The incremental benchmark already had the corresponding
release sequence and retains the same field meaning.

Segment inventory now comes from a crate-private iterator over the supplied
pinned segment groups. `IndexView::index_observation` folds those summaries
directly with no intermediate vector. Its new
`index_observation_with_segments` visitor shares that same traversal with
`bench_build_musenalm`, which collects only the requested segment tuples and
uses the observation's gram and inventory totals. `segment_summaries()` still
collects where callers explicitly request the complete listing.

The default construction benchmark obtains detailed inventory storage only
when `--storage-diagnostic` is enabled. Its incremental mode accumulates file
count, source bytes and fingerprint inputs in one catalogue-row traversal.
The build-owned bitmap field is now named
`build_empty_array_bitmap_serialized_bytes` and documents that it measures the
completed source generation's empty-array bitmap, excluding newly published
segments. Independent benchmark measurements with a different scope retain
their existing names.

The new regression
`segment_observation_visits_match_pinned_collection_aggregates` covers two
collections, an add that creates another segment, deletion of an existing file,
and three simultaneously pinned views. It verifies visitor order, every
segment-derived collection aggregate, aggregate totals, and the retained
build-bitmap size after another segment is published.

### Validation

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed. |
| `cargo check --locked --workspace --all-targets` | Passed. |
| `cargo test --locked -p objekte-search-core --all-features` | Passed: 583 unit tests, 16 ignored, plus core integration and doc tests. |
| `cargo test --locked --workspace` | Passed. |
| `cargo test --locked -p objekte-search-core segment_observation_visits_match_pinned_collection_aggregates` | Passed. |
| Pinned-count, shared-file, locator-deletion and ownership regressions | Passed in the core suites. |

Both benchmarks ran in release mode on the same two-file JSON fixture with two
workers:

```sh
target/release/examples/bench_index_construction --root /tmp/objekte-diagnostic-accounting.yvJLe8/fixture --result-dir /tmp/objekte-diagnostic-accounting.yvJLe8/default-results --plain --alphabet ascii --workers 2 --storage-diagnostic
target/release/examples/bench_index_construction --root /tmp/objekte-diagnostic-accounting.yvJLe8/fixture --result-dir /tmp/objekte-diagnostic-accounting.yvJLe8/incremental-results --plain --alphabet ascii --workers 2 --incremental
```

Both runs produced two files, two records, 73 source bytes, two successful
parses, zero failed parses, and corpus fingerprint
`0ea5b8236cac2fe01f38e466c7bdacdfb62092f06907a295c115a28fe2df17c6`.
Both reported 792 estimated index bytes, 543 inventory heap bytes, two bigram
stores and twelve distinct gram keys. The default detailed run emitted the
inventory storage breakdown. Default build time was 3,103,520 ns; incremental
build time was 3,412,689 ns. Their `after_build_rss` / `after_release_rss`
samples were 10,252,288 / 10,346,496 and 10,457,088 / 10,579,968 bytes.
The release sample is allowed to remain high because the allocator may retain
freed pages; no trimming or fall assertion was added.

Source order confirms the default view and owner drops at lines immediately
preceding its release sample, and the incremental view/cursor/owner/task drops
immediately preceding its release sample. Temporary benchmark data was removed.
No full-corpus timing suite was needed because this task changes diagnostic
traversal and benchmark sampling, not the construction timing interval. The
integration branch and worktree are retained for the owner's merge decision.
