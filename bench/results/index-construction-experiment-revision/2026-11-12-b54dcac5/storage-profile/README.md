# Opt-in full-corpus construction storage

Control B `54dcac5c3b` used a temporary uncommitted instrumentation patch, archived as `baseline-instrumentation.patch.gz`; the clean control checkout and ordinary timing binary were restored afterward. Instrumented B binary SHA-256: `a11c28cf574c15a4f7e2d18454ef4145c8adb0a96f20d8982f365a021f35e1aa`. Candidate `d7534b6d` has committed opt-in counters and release binary SHA-256 `b8e0524dbffdcfae56fb36d929b6e38ae36b75f9e0987962df77779888df2b71`. Both used Rust 1.99.1, locked release dependencies, the canonical Musenalm root, 16 workers, and `--profile`. Candidate additionally set `OBJEKTE_BIGRAM_STORAGE_STATS=1`; B's temporary patch samples automatically with `--profile`. These runs are excluded from ordinary timing medians.

Both roles report 6,807,376 final gram blocks and 25,477 distinct final keys in ASCII, and 7,248,099 blocks and 37,909 keys in Unicode. B's per-gram vectors have 16-byte blocks, 24-byte vector descriptors, 48-byte hash-map stores and a 104-byte contribution. The candidate has 16-byte blocks, 16-byte spans, 104-byte chunks, 24-byte pending entries and a 120-byte contribution; its global storage owns 302 chunks. Per-chunk spans total 1,111,671 ASCII and 1,249,051 Unicode, since a gram can appear in more than one batch.

| Alphabet | Role | Final global element-capacity lower bound | Peak ready lower bound | Process peak RSS | Post-build RSS |
| --- | --- | ---: | ---: | ---: | ---: |
| ASCII | B | 175,316,640 B | 18,558,336 B | 515.9 MiB | 478.1 MiB |
| ASCII | Contiguous | 129,252,704 B | 14,037,136 B | 453.4 MiB | 436.9 MiB |
| Unicode | B | 186,578,224 B | 21,256,832 B | 522.1 MiB | 504.7 MiB |
| Unicode | Contiguous | 145,299,088 B | 15,744,512 B | 469.2 MiB | 453.6 MiB |

The candidate packs/copies 108,918,016 ASCII and 115,969,584 Unicode block bytes across batch contributions. It then moves owned packed payload allocations into the global accumulator and stores each batch base once; it does not rebase every block on admission. Final conversion and destruction remain inside full-build timing. The candidate's observed maximum single-contribution scratch capacity was 177,168 B ASCII / 114,688 B Unicode, with 786,432 B peak pending capacity in both. These are maxima across contributions, not a simultaneous worker total.

Capacity totals omit hash control bytes, allocator rounding, active workers and completed channel results not yet read. Ready and global peaks were sampled separately; summing them would not give process peak memory. RSS and DHAT measure different boundaries. The final persistent index size is unchanged (69,662,968 B ASCII; 72,571,368 B Unicode) in the paired full builds.

## Detailed work accounting

`candidate-detailed/` is a later, separate opt-in profile of the retained integration code with `OBJEKTE_BIGRAM_STORAGE_STATS=1`, `--profile`, and 16 workers. It was written outside the measured worktree and moved here afterward; run-manifest commands preserve the executed path. The revision in these manifests includes a documentation-only evidence commit after the final timing binary was built; construction code is unchanged. The diagnostic box adds 1,080 B to the measured final global capacity lower bound, yielding 129,253,784 B ASCII and 145,300,168 B Unicode. It is absent from ordinary builds.

| Operation or stage | ASCII | Unicode |
| --- | ---: | ---: |
| Gram set lookups | 119,279,482 | 121,734,194 |
| Duplicate record-bit ORs | 63,003,574 | 63,676,107 |
| Entries sorted/grouped | 6,807,376 | 7,248,099 |
| Segment traversal, folding, gram emission and insertion | 754.914 ms | 2,015.117 ms |
| Scratch drain/clear/shrink | 93.760 ms | 99.186 ms |
| Sorting | 149.165 ms | 168.300 ms |
| Directory construction and block packing | 17.776 ms | 21.731 ms |
| Local and admission descriptor validation | 21.017 ms | 24.546 ms |
| Admitted payload move/ownership merge | 0.094 ms | 0.097 ms |
| Final bigram validation, header split, conversion and destruction wall interval | 151.676 ms | 154.697 ms |

The first eight timing rows are sums of opt-in service intervals across contributions and workers; they may overlap one another and other build work, and their sum is not wall or CPU time. The segment interval includes folding and key generation, so it is not an isolated hash-lookup duration. The finalization row is the existing wall boundary, not a sum of those service intervals. Baseline B did not expose this fine-grained stage breakdown; its separate profile exposes broader admission and finalization intervals. Ordinary paired full-build timing includes every stage, conversion and destruction.
