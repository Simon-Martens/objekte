# Objekte posting construction and projected inference — review and follow-up

Reviewed `repair/path-posting-query-evaluation` at `3f0c9f9f6c`. The final code change is `bfa53459b1`. The three counterexamples from the previous review are addressed. Global length admission takes the union of observed keys, null counts are bounded by projected presence, and posting costs no longer decrease at 33 matching keys. The tests exercise public length queries and production validation of projected and consolidated states. The branch reports 647 core tests passing, 16 ignored and eleven source-reference queries with zero file reads. This is a source review. There was no Rust toolchain here to repeat those runs. [Completion report](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/docs/ai/done/objekte_segmented_inference_and_posting_cost_repair.md#L78)

## Required correction for projected inference reports

The remaining defects affect the interpretation of the projected report. The examples do not establish incorrect query IDs. Apply the correction on the repair branch. The performance experiments below can proceed independently.

| Fixture and operation | Required result | Current calculation |
| --- | --- | --- |
| Auto-index a record with `{"items":[{"name":"A"},{"name":"B"}]}` and an unrelated second record. Retire the second record and compact. Inspect `items.name`. | One surviving record selects terminal scalars, zero select terminal arrays. Any advertised upper bound on scalar records must be at least one. | The builder sets `multi` for a path reached through an array. Projection maps `multi` directly to `scalar_records = 0` and `array_records = present_records`. A fresh report counts one scalar record. |
| Auto-index `{"x":1.5}` and `{"x":7}`. Retire the fractional record and compact. Inspect `x`. | A field marked complete must agree with the fresh report for the survivor, including `integer_only = Some(true)`. | Projection clones the old report and allows complete single-valued numeric coverage. It updates occurrence counts while retaining `integer_only = Some(false)` from the retired value. |

The first defect comes from using a property of the whole path to infer the type of a terminal value. The second comes from using a fact about the original population as an exact fact about a subset. [Builder counts and traversal](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/crates/core/src/construction/inference.rs#L1459) · [Multiplicity construction](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/crates/core/src/construction/inference.rs#L1774) · [Projected counters](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/crates/core/src/index/segment/projection/inference.rs#L117)

- For incomplete projected scalar/array counts, cap the corresponding inherited record counts by the surviving possible-present population, or use a tighter proved bound. A field can select both kinds in one record. Check the analogous empty-selection fallback when length coverage was withdrawn: the absence of a zero-length posting then cannot prove zero terminal empty arrays.
- Audit the fields preserved by the branch that still advertises a complete numeric report. Recompute a fact only when the retained evidence distinguishes it at acceptable cost. Mark the report incomplete or use the supported unknown representation if the remaining numeric representation cannot prove historical distinctions. Preserve exact query capabilities independently. Check the empty-survivor case, too.
- For both fixtures, compare the compacted report against a fresh build of the survivors, then check it again after consolidation. Require exact comparisons for complete fields and test the promised direction of each bound on incomplete fields. Keep the production validator assertions. A passing validator alone cannot detect a semantically wrong count that is in range.

## Construction measurements and their scope

| Comparison | Musenalm fresh ASCII build | What the measurements establish |
| --- | --- | --- |
| Initial path-posting redesign | 0.752 s to 0.688 s, three samples per revision | 8.5% lower median and 52.4 MB less reported resident memory in that comparison. |
| First repair | 0.715 s to 0.708 s, three samples per revision | 7 ms difference with overlapping samples. The rerun of the earlier revision already gave a different median. |
| Latest repair | No new construction timing | It fixes correctness and costing. The deduplicated length-key union changes opt-in inference construction. Its CPU or allocation cost is unmeasured. |

The reported bench command selects ASCII bigrams and disables the verified query cache. The example uses default repository inference, which is disabled. The default build still discovers and reads files, extracts values, builds bigrams, sort and relation indexes, and seals segments. Removing structural bitmaps changes only part of that path. The four query workers in the report do not specify the construction worker count. [Initial measurements](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/docs/ai/done/objekte_posting_index_and_query_execution_specification.md#L284) · [First repair](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/docs/ai/done/objekte_path_posting_query_repair_specification.md#L148) · [Benchmark options](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/crates/core/examples/bench_musenalm.rs#L311) · [Default repository options](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/crates/core/src/repository/configuration/mod.rs#L92)

## First experiment on the measured build — transfer the first batch’s posting maps

Workers already build a complete local `PostingCollection`. Segment sealing creates an empty aggregate and merges every batch. For declared postings the merge drains and sorts the destination field map, pairs fields with the incoming map, makes job vectors, and reconstructs the map. The bitmap helper enumerates individual slots even at base zero. For the first batch of a segment the IDs already have their final segment-local values. In that case the incoming field maps and Roaring containers can be transferred by ownership. This is avoidable work even if its share of wall time turns out to be small. [Segment sealing](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/crates/core/src/construction/segment_admission.rs#L777) · [Field-map reduction](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/crates/core/src/index/posting/mod.rs#L517) · [Bitmap append](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/crates/core/src/inventory/mod.rs#L22)

Start with declared postings only. Transfer the incoming collection when the destination is the untouched, compatible collection and the slot base is zero. Keep the existing final validation and the shifted merge for subsequent batches. An empty presence bitmap by itself does not prove the destination is untouched. Cover missing fields, zero logical records, selected nulls, structural-only declarations and a second nonzero-base batch. Count zero-base transfers, keys and bitmap members whose replay was avoided. Compare segment aggregate merge time and end-to-end build time.

## Second experiment on the measured build — avoid repeated field selection and temporary keys

For every logical record, `PostingCollection::add` evaluates every declared field path from the root. Sibling fields below the same array independently traverse its elements. Once a path is resolved, `add_values` observes the selection, tests for any null, tests whether all are null, then takes the singleton path or builds a temporary scalar-key hash set for a general multivalue selection. Constructing string and ID keys also clones their text before checking whether the dictionary already has the key. The shape and cardinality of the data determine which cost dominates. [Per-field selection](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/crates/core/src/index/posting/mod.rs#L580) · [Selected-value processing](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/crates/core/src/index/posting/mod.rs#L369) · [Key construction](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/crates/core/src/index/posting/mod.rs#L30)

First measure singleton, short-array and repeated-prefix populations. Specialize singleton handling before general scans, and compare direct bitmap insertion for small arrays against the current temporary set. Duplicate bitmap insertions are idempotent, but repeated B-tree lookups on long duplicate-heavy arrays can be expensive. Keep both workloads in the comparison. Consider a compiled shared-prefix traversal only if profiling shows substantial repeated path walks. Preserve successful empty traversal, missing terminals, multiple terminal nulls and array-element versus terminal-null semantics.

## Separate experiment with auto-indexing enabled — remove transient posting copies

At segment finish, `observation()` first deep-clones the vector of field states. Those states include scalar and short-string dictionaries, length dictionaries, and their Roaring postings. The function immediately discards the cloned value dictionaries and converts the cloned length map to a set of keys. The original builder is then consumed by `finish_candidates`. The report’s statement that it moves the length keys describes the move from this temporary copy. [Observation construction](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/crates/core/src/construction/inference.rs#L1917) · [Segment finish caller](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/crates/core/src/construction/segment_admission.rs#L869)

Construct the observation from the fields the global admission pass actually reads. Copy bounded keys and required evidence once and leave posting bitmap ownership with the builder. Consider an ordered vector for unique per-segment length keys already emitted in order. Preserve the collection-wide bounded union and test overlapping and disjoint lengths. Run the comparison under AutoIndex and include a high-cardinality string field. Measure inference finish time, allocations or peak RSS, and total build time. This optimization cannot explain the timings of an inference-disabled run.

## Verification budget

Use separate baseline and candidate release binaries on the same warm-cache corpus, with identical schema, alphabet, resolved build workers and segment policy. Alternate a few fresh-process runs and record individual times. Use profiling runs to explain the result and uninstrumented runs for end-to-end comparisons. Record both build and open-to-complete timings when available. Existing counters cover posting construction, aggregate merge, posting validation, inference finish, discovery and source extraction. Sampled worker times overlap and must not be added to wall time. The archived Musenalm profile at `8d4cfdc4c7` is historical context only. It measured about 240 ms median discovery and separately sampled 2,321 construction records. Timing a representative build at the current code revision is necessary before assigning it a savings percentage. [Historical measurements](https://github.com/Simon-Martens/objekte/blob/3f0c9f9f6c/bench/results/index-creation-loading-profiling/summary.json)

## Implementation report

Task identifier: `projected-inference-and-construction-review`

Completion date: 2026-11-20

Base commit: `3f0c9f9f6c453e950a7414bae8748481c2c6225a`

Branch: `repair/path-posting-query-evaluation`

Worktree: `/home/simon/source/objekte-repair-path-posting-query-evaluation`

The task explicitly requested continuation on the reviewed repair branch, so
no separate integration branch or integration worktree was created.

### Implemented changes

- Projected terminal record counts now use the observed terminal kinds and
  surviving present bound instead of mapping path multiplicity directly to
  scalar or array records.
- Withdrawn length coverage now leaves an empty-selection bound instead of
  interpreting the absence of a zero-length posting as proof of zero empty
  terminal arrays.
- Complete projected numeric reports recompute `integer_only` from retained
  numeric value keys. Incomplete numeric reports use `None`, and the
  empty-survivor case does not retain the retired population's answer.
- The production inference validator rejects scalar, array, object,
  empty-array, empty-selection, and nonblank counts that exceed their surviving
  bounds.
- Tests compare compacted projected reports with fresh survivor builds and
  repeat the comparison after consolidation for both reported fixtures.
- Declared postings transfer an untouched compatible first batch by ownership
  at base zero. The shifted merge remains in use for later batches, and profile
  counters record transfers, keys, and bitmap members.
- Singleton scalar handling remains on its direct path. General multivalue
  selections of at most eight flattened values insert directly instead of
  constructing a temporary set; larger selections retain the deduplicating
  fallback.
- Inference observation copies only the fields needed by collection-wide
  admission and leaves the builder's value, short-string, and length posting
  containers in place.

### Commits

- `9ed2c1a189650a2b4afed07862a5dc9e972db641` correct projected terminal and
  numeric inference facts.
- `019d64e84678828d1e72918889604e1ab721c7c0` transfer pristine postings and
  specialize small selections.
- `829993ce8c` avoid transient inference observation copies and add the posting
  population benchmark.
- `9281f95b47` format construction review changes.
- `b80e5fad87` complete consolidation, empty-survivor, validator, and
  duplicate-population coverage.
- `582c619a12` record the construction and correctness measurements in
  [the generated results](../generated/projected_inference_and_construction_repair_results.md).

### Validation

- `cargo test -p objekte-search-core --lib --locked projected_ -- --nocapture`:
  seven selected projected-inference tests passed.
- `cargo test --workspace --locked --no-fail-fast`: passed. Core ran 652 library
  tests with 16 ignored; CLI, desktop, all integration suites, and doctests
  passed.
- `cargo check --workspace --all-targets --locked`: passed.
- `cargo check -p objekte-search-core --all-targets --features
  detailed-profiling --locked`: passed.
- `cargo clippy --workspace --all-targets --locked`: completed without errors
  and retained existing warning-level diagnostics. New warnings in touched code
  were removed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

### Benchmarks

Five alternating fresh-process pairs used separate ordinary release binaries,
the warm 148,486-record Musenalm ASCII corpus, eight construction workers, and
the verified cache disabled. Inference-disabled median build time was neutral
within noise (887.696 ms to 887.385 ms). AutoIndex median build time was
1,595.941 ms to 1,561.403 ms, and peak RSS was 894.9 MiB to 869.2 MiB.

All 15 profiled segment jobs used the pristine transfer path, avoiding replay of
19,565 keys and 410,403 bitmap members. Sampled aggregate merge work fell from
996.881 ms to 915.033 ms without inference and from 1,960.790 ms to 1,808.533 ms
under AutoIndex. Sampled AutoIndex inference finish time fell from 808.905 ms to
664.152 ms. Because segment jobs overlap, these sampled totals are not wall
time.

Seven fresh processes per revision measured declaration insertion. The median
of process medians fell from 15.882 ms to 14.229 ms for singleton records and
from 11.033 ms to 9.191 ms for short arrays. Repeated-prefix and duplicate-heavy
workloads were effectively unchanged.

Full commands, per-run values, binary hashes, environment, and interpretation
are in the generated results report.

### Remaining limitations

The inference-disabled end-to-end timing is neutral within run variance. The
AutoIndex timing direction is favorable but includes one candidate outlier, so
the sampled counter reduction is stronger evidence than that wall-time ratio.
The direct fresh-build profiler rejects overlapping nested extraction totals in
both revisions; profiling used its incremental reporting path with the same
construction code, while all end-to-end measurements used uninstrumented fresh
processes.

No subordinate worktrees were created. The repair worktree and branch are
retained for the user.
