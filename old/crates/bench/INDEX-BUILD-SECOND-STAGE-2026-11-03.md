# Second-stage index construction results — 3 November 2026

All figures are medians of five fresh, instrumented release builds on the same source directories. The 32-worker runs used largely warm filesystem caches. The starting revision was `0175da51f5`. Each profile saved and restored a snapshot and checked that restored index sizes matched the built index. Raw per-step profiles are in [the step results](results/index-build-second-stage-steps-2026-11-03.ndjson); final profiles are in [the final results](results/index-build-second-stage-2026-11-03.ndjson).

## Decisions by step

| Step | Decision | Representative build time | Peak RSS | Finding |
|---|---|---:|---:|---|
| 4. Stable value-only view ordering | Retained, `75cfb67153` | Musenalm base 1,056 → 1,029 ms | 382 → 384 MiB | View ordering itself fell 407 → 380 ms. Stable sorting preserves ID ties because fresh and compacted slots are ID ordered. The ID-aware comparator remains available for unordered slots. |
| 5. Rayon task grouping | Rejected | Leipzig FTS at 32 workers: 3,301 ms with 1 chunk, 3,345 ms with 8 | Similar | Five runs of each 1/2/4/8/16-chunk setting at 8, 16, and 32 workers showed no reliable total-build gain. The experimental code was reverted. |
| 1. Partition global rendered postings | Retained, `b85f1ea870` | Leipzig FTS 3,314 → 2,223 ms | 2,931 → 2,292 MiB | Registered fields no longer duplicate their rendered text in the global index. Complete candidate sets are unioned across partitions. Observed-gram maps retain selectivity when a partition has no compressed constraint. Sidecar version `OBJSEC06` rejects the old combined representation. |
| 3A. Read UTF-8 columns directly during compression | Retained, `07c85da7ea` | Leipzig FTS 2,223 → 1,649 ms | 2,292 → 1,688 MiB | Compression no longer copies every temporary UTF-8 column into a second full slab. It allocates final flat storage only for dense columns. Compressed size stayed 73.5 MB. |
| 2. Aggregate postings per 64 slots | Retained, `aec820ad54` | Amazon FTS 3,113 → 3,003 ms; NYC FTS 3,202 → 2,859 ms | Amazon 3,574 → 3,615 MiB; NYC 1,905 → 1,942 MiB | Main ASCII insertion uses reusable chunk masks. Field masks are used from 250,000 files upward; on Musenalm they added memory without build-time gain. Main posting-insertion worker CPU fell 846 → 535 ms on Musenalm base and 6,976 → 3,772 ms on Amazon base. |
| 3B. Sparse-first UTF-8 construction | Retained, `1d86487a36` | Leipzig FTS 1,616 → 1,192 ms | 1,698 → 888 MiB | Chunk-local maps publish nonzero 64-slot blocks. Compression promotes only columns whose encoded sparse form would be larger than dense storage. The final `BigramFilter` and snapshot formats remain unchanged. |

The UTF-8 distribution justified step 3B. In Leipzig's two normal field builders, about 11,880 distinct pairs were observed per filter; only 107 columns finished dense. The skip-one builders observed about 15,940 pairs each but retained only 103 columns after the density cutoff. Their median observed column had 65 nonzero blocks out of 6,250 possible blocks.

## Combined results

These are the four configured FTS profiles. Sizes are compressed bigram-index memory in decimal MB; RSS is peak process resident memory during construction.

| Collection | Files | Build before → after | Peak RSS before → after | Index before → after | Final snapshot save / restore |
|---|---:|---:|---:|---:|---:|
| Musenalm | 133,948 | 1,280 → 1,226 ms (−4%) | 773 → 778 MiB | 52.7 → 49.7 MB | 356 / 421 ms |
| Leipzig | 400,000 | 3,314 → 1,192 ms (−64%) | 2,931 → 888 MiB | 109.6 → 73.5 MB | 718 / 315 ms |
| Amazon | 1,000,000 | 3,398 → 2,989 ms (−12%) | 3,974 → 3,595 MiB | 430.5 → 341.8 MB | 2,017 / 1,042 ms |
| NYC 311 | 1,000,000 | 3,317 → 2,876 ms (−13%) | 1,942 → 1,944 MiB | 379.6 → 354.9 MB | 1,952 / 967 ms |

The final base-only builds were 1,015 ms / 385 MiB for Musenalm, 1,028 ms / 513 MiB for Leipzig, 2,125 ms / 1,275 MiB for Amazon, and 2,173 ms / 776 MiB for NYC. Their compressed indexes were 15.8, 36.5, 147.9, and 110.9 MB respectively.

## Query and correctness checks

| Workload | Before sum of case medians | Final sum | Result check |
|---|---:|---:|---|
| Musenalm JSON, 100 cases | 5,408 ms | 4,154 ms | All counts and first IDs identical; no final case exceeded its starting median by 30%. |
| Musenalm FTS, 30 cases | 565 ms | 580 ms | All counts identical; no final case exceeded its starting median by 30%. An immediate repeat totaled 571 ms. |
| Leipzig multilingual JSON, 50 cases | 517 ms immediately before sparse-first | 404 ms | All counts, first IDs, candidate scans, and source reads identical. Timing varied materially with cache state, so this is a non-regression check rather than a search-speed claim. |

The [final JSON](results/index-build-second-stage-json100-2026-11-03.ndjson), [final FTS](results/index-build-second-stage-fts30-2026-11-03.ndjson), and [final Leipzig](results/index-build-second-stage-leipzig50-2026-11-03.ndjson) case results are saved beside their baseline files. Source-oracle tests compared complete match sets against direct source evaluation; the benchmark signatures alone were not used as the correctness proof. The full core and workspace tests passed, including ASCII and UTF-8 candidate equivalence, Unicode and markup searches, malformed files, live overlays, and snapshot restoration. The ignored Musenalm bigram and JSON source-oracle tests also passed. Workspace Clippy passed with warnings denied.

Changed Rust files pass the repository's custom rustfmt settings (`blank_lines_upper_bound=2`). Default `cargo fmt --all -- --check` still reports existing formatting differences across unrelated bench, CLI, and core files.

## Remaining limits

Sparse-first UTF-8 construction still merges one touched key at a time under a per-column lock when each 64-slot chunk is published. This is much less frequent than per-document locking but remains a likely scaling limit above the tested worker count. Allocation failure during sparse-column setup falls back to the previous dense builder; failure while growing a sparse posting list makes that filter unconstrained rather than risking false negatives. The ASCII field accumulator is deliberately disabled below 250,000 files because the measured Musenalm build showed no wall-time benefit from its extra scratch memory.
