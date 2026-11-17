# rkyv construction performance follow-up

## Result

The requested construction-work removal and correctness repairs are
implemented on `integration/rkyv-memory-index-repair`.

The result is **correctness complete; performance acceptance not complete**:

- all required lifecycle, publication, verification, query-semantic and
  workspace tests pass;
- the follow-up is materially faster than the repaired predecessor;
- no supported construction configuration reached native parity;
- query aggregate safeguards and one focused direct-query threshold remain
  above their limits.

No architecture, segment policy, inference policy, query semantics, public
lifecycle behavior or archive format feature was relaxed to obtain the timing
result.

## Revisions and environment

| Role | Revision |
| --- | --- |
| Native baseline | `2e18871c0bcfb9cf9335d9e73525c64f91c960cf` |
| Completed predecessor repair | `e4a7471ffe777a9a4992dff34d7070edf372e873` |
| Follow-up implementation | `65cb66dc5b` plus the final documentation commit |

The final construction matrix was taken from commit `65cb66dc5b` with the
follow-up executable rebuilt after that commit. Exact hashes, lockfile digest,
toolchain, corpus, worker settings and target directories are in
[`revisions.json`](revisions.json).

The primary corpus was `examples/musenalm`: 7 collections, 148,486 logical
records, 133,948 files in the largest collection, default 16,384-record and
16,384-file segment limits. Every sample used a fresh process and warmed source
page cache. Round order alternated N/F/R and R/F/N.

## C01–C15 outcomes

| ID | Outcome |
| --- | --- |
| C01 | Fixed. `OrderedPostingArchiveSource` and `OrderedPostingMapSource` serialize the checked ordered entries directly. Freeze no longer creates a second native ordinal B-tree or clones keys into it. |
| C02 | Fixed for production insertion. `StoredScalarKey::from_value` performs one direct conversion and owned public keys move through consuming conversion. A reusable scratch lookup was benchmarked but rejected because high-cardinality insertion regressed 16.9%. |
| C03 | Fixed. The first batch is adopted as owned families after schema/configuration checks. Structurally compatible first maps move directly; no bitmap-member enumeration occurs. |
| C04 | Fixed with the simpler required path. Later batches merge into the existing `HashMap` in place. No per-batch field sort/container reconstruction is performed. |
| C05 | Fixed. The collection compatibility header is built with `SortCollection::configuration_only`; all payload vectors start with zero length and zero capacity. |
| C06 | Fixed for row and lookup intermediates. File diagnostics are consumed/moved, record IDs are moved when unique, and record lookup serialization uses an ordered borrowed key/reference vector rather than an owned native string map. Lossless paths are transformed once. |
| C07 | Reduced. Adopted indexes reuse current catalog owners instead of rebuilding the complete catalog. Identity/index maps that remain are required public compatibility state; row caches stay lazy. |
| C08 | Fixed. Base and segment relation dictionaries are frozen and validated before publication. A relation fixture asserts all reachable base posting dictionaries are archived. |
| C09 | Extended. Existing build profiling now attributes adoption, archive freeze, row serialization, validation, catalog work, duplicate probes and temporary registry bytes. Detailed-profile evidence is retained. |
| C10 | Fixed. Catalog rebase compares archived rows with the final inventory, rebuilds only changed metadata shards through the existing pool, rebuilds zero unchanged path shards, and preserves ownership routes and owners. |
| C11 | Fixed. Bitmap-only validation uses `for_each_bitmap`; domain checks are fused into the single completion pass; an already completed sealed segment immediately reuses its completed state. |
| C12 | Fixed on the healthy path. Initial publication checks borrowed status/path/revision fields and batches actual diagnostic edits by metadata shard. Healthy files materialize no native file or segment row. |
| C13 | Fixed for inference-only rebuilds. File/record archives, sorting, raw text, fallback text and custom-text maps are shared through immutable owners. Inference pruning thaws only the value dictionary and retains the exact empty-string bitmap directly. Status edits are handled separately. |
| C14 | Fixed and bounded. New IDs use an allocation-free empty-base check and a construction-only exact-ID registry; the registry is released before final publication and duplicate probes/bytes are counted. |
| C15 | Fixed. Initial and final repository shells use a private configured-state constructor that reuses bindings while avoiding construction-pool creation and repeated empty finalization. |

## P01–P24 coverage

| Tests | Executed evidence |
| --- | --- |
| P01–P04 | Existing empty/singleton/4,096-key ordered archive tests; `ordered_posting_source_rejects_unordered_duplicate_or_sparse_entries`; `posting_freeze_serializes_one_owned_ordered_directory`; checked archive and semantic-corruption regressions. |
| P05–P07 | Direct conversion and multi-value lifecycle tests; `reusable_lookup_scratch_microbenchmark`; all existing exact-number, array, null, empty-selection and completeness tests. |
| P08–P10 | `admits_one_worker_batch_spanning_files_without_reprojecting_postings` asserts one first-batch transfer and zero field-container rebuilds; multi-batch/worker inference and mutation tests pass. |
| P11 | `configuration_only_sort_header_has_no_payload_capacity`; configured collation, ties and page tests pass. |
| P12 | Row/lookup tests cover Unicode and diagnostics; archive serialization is completed before source rows are dropped. Lossless path tests cover non-UTF-8 input. |
| P13–P14 | Cache-free build, lazy/shared compatibility caches, storage diagnostic and pinned-view tests pass. |
| P15 | Relation finalization test asserts all base posting dictionaries are archived and exercises update/delete/compaction behavior. |
| P16 | Existing archive-finalization failure, cancellation and drained-worker tests pass. |
| P17 | Deterministic five-seed lifecycle matrix compares fresh rebuild and independent model after every add/replace/delete/compaction operation. |
| P18 | Repeated build/drop and pinned-old-view owner-release tests pass. |
| P19 | Catalog rebase regression proves unchanged path/ownership owners survive; immediate post-publication mutation tests pass. |
| P20 | Sealed-component reuse and bitmap-only validation regressions pass; validation-owned key bytes remain zero. |
| P21 | Healthy initial publication regression records zero native file/record compatibility copies and zero cache initialization. |
| P22 | Inference lifecycle and query tests pass with shared native sidecars and preserved pinned views. |
| P23 | Duplicate construction tests cover files, batches and registry cleanup; probe and registry-byte counters are present. |
| P24 | `configured_empty_shell_preserves_collection_bindings_without_build_pool` covers empty-file state, bindings, archived postings and query-worker lifetime. |

## Construction benchmark

Seven paired samples per cell and revision, printed as median milliseconds:

| Alphabet | Workers | Native | Predecessor | Follow-up | Follow-up/Native | Gate |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| ASCII | 1 | 3,671.0 | 4,177.1 | 4,016.4 | 1.094x | failed |
| ASCII | 8 | 808.0 | 1,192.5 | 987.6 | 1.234x | failed |
| ASCII | 32 | 649.4 | 1,073.4 | 832.1 | 1.287x | failed |
| Unicode | 1 | 4,393.9 | 5,047.7 | 4,775.5 | 1.085x | failed |
| Unicode | 8 | 930.8 | 1,345.5 | 1,099.2 | 1.173x | failed |
| Unicode | 32 | 702.9 | 1,140.4 | 897.6 | 1.270x | failed |

The paired median and 95% bootstrap interval for every row are in
[`build-summary.tsv`](build-summary.tsv). Raw samples are in
[`build-samples.jsonl`](build-samples.jsonl).

The follow-up improves predecessor median construction by approximately 17% at
8 workers and 19–22% at 32 workers. The 32-worker ASCII case moved from
1,073.4 ms to 832.1 ms. The native target remains unmet.

The current benchmark harness exposes one `Repository` construction entry path,
which internally exercises progressive publication. A separate direct versus
progressive construction matrix was therefore not available; this is recorded
as unmeasured rather than passed.

## Query benchmark

All 150 cases completed through direct and progressive paths for ASCII and
Unicode. Native/follow-up and predecessor/follow-up semantic digest mismatches
were zero for every case.

One-repetition aggregate sums:

| Mode | Native | Predecessor | Follow-up | Follow-up/Native |
| --- | ---: | ---: | ---: | ---: |
| Direct, combined | 2,694.2 ms | 2,870.1 ms | 2,840.7 ms | 1.054x |
| Progressive, combined | 2,645.4 ms | 2,738.5 ms | 2,769.1 ms | 1.047x |

The 3% aggregate query safeguard failed. A focused three-repetition retry
reduced the initial cold Q001 outlier, but direct Unicode Q119 remained
`7.166 / 4.607 = 1.555x` native and progressive Unicode Q119 remained `1.626x`,
above the individual threshold. These are recorded as failed safeguards in
[`query-summary.json`](query-summary.json).

## Memory and ownership

The follow-up reduced retained process RSS versus the predecessor in every
construction cell:

| Cell | Predecessor end / peak MiB | Follow-up end / peak MiB | Native end / peak MiB |
| --- | ---: | ---: | ---: |
| ASCII 1 | 606.7 / 671.9 | 555.5 / 620.9 | 415.9 / 458.3 |
| ASCII 8 | 673.0 / 737.1 | 590.8 / 655.1 | 462.6 / 503.5 |
| ASCII 32 | 839.6 / 902.2 | 721.6 / 784.8 | 575.2 / 615.6 |
| Unicode 32 | 846.5 / 909.3 | 716.6 / 779.7 | 596.4 / 636.7 |

Unique archive owners fell from 4,757 to 2,594 after empty-archive sharing and
catalog reuse. Exact archive capacity, slack and native payload categories are
in [`metadata-ownership.md`](metadata-ownership.md) and
[`allocation-summary.tsv`](allocation-summary.tsv). File/record row caches stay
uninitialized during construction.

## Experiments and residual cause

The reusable posting-key scratch lookup was faster on repeated keys
(`4.557 ms` versus `7.030 ms`) but slower on all-unique keys (`37.812 ms`
versus `32.349 ms`, +16.9%). Production retains one-search direct insertion.

Detailed profiling shows the remaining 32-worker gap is dominated by segment
finalization rather than discovery:

- one large contents shard spends roughly 186 ms rebuilding/packing bigram
  indexes;
- archive freeze/validation and segment-row construction remain serialized
  within each critical-path shard;
- coordinator record inventory and final compatibility-generation work remain
  measurable serial stages;
- required public compatibility data and native bitmap/text/sort sidecars remain
  resident.

The next concrete optimization would be to overlap bigram packing with posting,
lookup and row-archive finalization for the same shard while keeping the
existing bounded scheduler. That is a substantial ownership restructuring and
was not attempted after the established performance gate had already failed.

## Validation

| Command | Result |
| --- | --- |
| `cargo test -p objekte-search-core --lib --locked` | 584 passed, 17 ignored, 0 failed |
| `cargo test --workspace --locked --no-fail-fast` | passed |
| `cargo test -p objekte-search-core --test compaction_api --locked` | passed |
| `cargo check --workspace --all-targets --locked` | passed |
| `cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked` | passed |
| `git diff --check` | passed |

The ignored tests are existing manual profiling/scale cases plus the new
manual reusable-key microbenchmark.

## Remaining limitations

- Native construction parity was not reached in any measured construction cell.
- Aggregate query time remains 4.7–5.4% above native.
- Focused Q119 Unicode direct/progressive queries exceed the individual query
  threshold.
- Construction was measured through the available unified progressive entry
  path; a distinct direct-construction matrix is not exposed by the harness.
- First progressive publication latency was not emitted by the current harness.
- The mutation benchmark was not rerun because the full deterministic
  matrix and workspace suites completed instead; no new mutation work was added.
- The follow-up remains unmerged on the integration branch for review.
