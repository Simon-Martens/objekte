# Verified base maintenance rewrites a large collection

- **Category:** perf
- **Originating task:** `index-lifecycle`
- **Observed branch:** `integration/index-lifecycle`, after `20c04e23cd`
- **Code:** `crates/core/src/lifecycle_verified.rs`, `crates/core/src/lifecycle_compaction.rs`

## Observation

On three disposable copies of the 148,486-file Musenalm corpus, one `UpdateStrategy::PreferVerified` update of an original base file took a median 1,278.918 ms (range 1,246–1,300 ms). It physically removed the old contribution as requested. The same fixture's ordinary appended update took a median 0.329 ms. The verified base phase raised process high-water memory from about 0.59 GiB to about 1.05 GiB; the full stress process later peaked at 1.37–1.41 GiB. The full commands, corpus, and raw output paths are in `docs/ai/generated/index-lifecycle-performance-validation.md`.

The verified path compacts a private staged root before one atomic publication. That gives correct physical removal and pinned-view safety, but the work scales with the collection. The current implementation caps this path at 262,144 scanned records per collection and reports a safe fallback beyond the limit. Ordinary updates remain sparse and fast.

## Expected behavior and impact

`PreferVerified` should ideally remove one old file's contributions with work proportional to that file and its index entries. At present, callers choosing it for low-latency updates may see second-scale latency and a large temporary memory peak on sizable collections. This does not affect the default appended update strategy.

## Reproduction

Run the committed `bench_lifecycle_mutations` example in `stress` mode on a fresh disposable Musenalm copy with `OBJEKTE_BENCH_MUTATE_COPY=1`; inspect the `update_verified_base` phase. The benchmark creates no source changes outside its supplied copy.

## Possible approaches

- **Current safe path:** Use ordinary appended replacement for interactive updates and compact explicitly later. This is measured and already supported.
- **Potential future design:** Partition base contributions into independently replaceable file groups, or add bounded reversible per-file posting ownership to the base. This may reduce verified update work but needs its own implementation and memory/performance evaluation.

The task's required verified physical-removal behavior is present; this report records its measured cost for future user review. It does not authorize the possible redesign.

## Resolution in `index-lifecycle-repair`

The repair replaces the collection-sized staged compaction with bounded base shards and slot-preserving physical replacement for eligible built-in one-record files. All three independent Musenalm stress runs selected `VerifiedSlotPreserving` for both touched and untouched original-base files, with no fallback. The original-base verified update median fell to 45.585 ms (44.925–45.840 ms); the untouched original-base median was 40.416 ms (39.040–40.747 ms). The historical 1,278.918 ms collection rewrite is no longer on this eligible path. A 300,000-physical-file/450,000-logical-record scale test and source-free compaction above one million active records also passed, eliminating the old 262,144-slot ceiling for these operations. See `docs/ai/generated/index-lifecycle-repair/validation.md` and its preserved raw `admission-final-{1,2,3}.tsv` results.

This resolves the reported collection-sized rewrite, but the operation has not reached the specification's single-digit-millisecond engineering target. Profiling of the three final runs attributes about 15–16 ms to affected-shard compaction, 3–4 ms to consolidation, and 12–13 ms to dense remapping. The 2,048-record shard budget preserves near-baseline fresh-build time (1,321.780 ms versus 1,317.728 ms historically) while making these remaining costs local to bounded affected data. Ordinary appended updates remain the default; their current median is 0.561 ms versus 0.329 ms historically. These are measured tradeoffs, not a claim that every source and extension uses the local verified route.

The remaining bounded-path latency is tracked separately in `docs/ai/issues/perf/verified-base-update-bounded-path-latency.md`.
