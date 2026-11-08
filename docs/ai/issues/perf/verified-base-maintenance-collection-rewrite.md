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
