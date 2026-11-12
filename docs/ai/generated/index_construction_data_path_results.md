# Index construction data-path results

Task `index-construction-data-path`, 12 November 2026. Baseline B is `ab616dc6aabb9b9fee4657deb6752a6512bec9e5` on remote `master`; it includes the selected construction merge, root-relative path/shared-ID work, and the later archived-comparison cleanup. All experiments forked from B. The final integration branch retains direct scalar ASCII folding. SIMD, compact contribution storage, and parallel admission were implemented and measured on their pushed experiment branches, then excluded from final code for the reasons below. The original checkout remains on `master`.

[Raw evidence and exact commands](../../../bench/results/index-construction-data-path/2026-11-12-ab616dc6aa/README.md) include per-run manifests, binary hashes, individual trials, profiles, DHAT files, query checks, and validation logs.

## Method and identity

The host was an Intel Core i9-13900K with 32 logical CPUs, Linux `7.2.15-200.fc44.x86_64`, a `performance` governor, and affinity `0-31`. Rust was `1.99.1` with locked dependencies. Release binaries used separate target directories. The Musenalm corpus had 148,486 files and 193,297,230 source bytes; inventory fingerprint `9f93a4ba7fb9249c9c4eb04e03ed911435a6b0493f15b087a5447add32456aa9`, schema fingerprint `4115070eb30b0af98b109429d0883b493f857cdf671ec55b943d91b73c5ebfe3`. All ordinary timing comparisons used the existing `compare.py` driver and `bench_index_construction` example, one warm-up per role, alternating fresh-process pairs, 32 resolved workers unless labeled otherwise, and no profiling. B's binary SHA-256 was `d9f29ad155893c6e6d22c0c88a21ec114f44953e01c4b7551a769da9e34daeef`; the final scalar-only binary was `42449be3ded342ff6228c88a52a4dddd200f6501314eff3da739803cb4be3c0b`.

Each number below is a paired candidate-minus-control value. Runs from different comparisons are not combined or added. Peak and post-build RSS are process measurements, not contribution-only memory.

## Independent experiments

| Candidate and alphabet | B median build | Candidate median build | Paired changes (ms) | Paired median | Median peak RSS B → candidate | Median post-build RSS B → candidate |
| --- | ---: | ---: | --- | ---: | ---: | ---: |
| Scalar ASCII folding, ASCII | 1,201.068 ms | 1,129.486 ms | −27.486, −75.317, −61.039 | **−61.039 ms (−5.146%)** | 574.4 → 588.6 MiB | 536.3 → 564.8 MiB |
| Scalar ASCII folding, Unicode | 1,263.259 ms | 1,250.777 ms | −26.314, +33.876, −50.682 | **−26.314 ms (−2.060%)** | 602.3 → 613.7 MiB | 585.9 → 596.8 MiB |
| Compact contribution arena, ASCII | 1,182.374 ms | 1,583.694 ms | +396.444, +375.618, +449.615 | **+396.444 ms (+34.460%)** | 591.7 → 515.3 MiB | 557.8 → 430.2 MiB |
| Compact contribution arena, Unicode | 1,247.880 ms | 1,710.112 ms | +513.062, +481.797, +334.750 | **+481.797 ms (+39.224%)** | 602.8 → 528.6 MiB | 585.2 → 435.0 MiB |
| Parallel admission, ASCII | 1,232.342 ms | 1,171.244 ms | −17.051, −76.088, −30.146 | **−30.146 ms (−2.509%)** | 589.1 → 585.2 MiB | 565.2 → 537.3 MiB |
| Parallel admission, Unicode | 1,291.148 ms | 1,247.149 ms | +0.954, −19.838, −43.999 | **−19.838 ms (−1.585%)** | 594.8 → 580.5 MiB | 541.0 → 554.6 MiB |

The scalar path reads borrowed bytes, lowers `A..=Z` directly, and invokes the locked full, non-Turkic case fold for complete non-ASCII scalars. One folded stream preserves ASCII-byte and Unicode-scalar pair histories across Unicode expansions; history resets at segment boundaries. The final `BigramIndex`, gram version, query folding, and snapshot format are unchanged. The branch `task/index-construction-data-path/ascii-folding` ends at `66c40c0edb` and was merged as `ab463da9e6`.

The native SIMD candidate is preserved at `d938fa5618` on that branch. Three release kernel trials emitted all adjacent and skip-one grams over short/long ASCII, ASCII-heavy mixed UTF-8, and predominantly non-ASCII inputs. AVX2 helped one mixed ASCII case by 9.8%, but slowed common 32–128-byte ASCII cases by 6.7–10.1% in the ASCII alphabet and predominantly Unicode input by 5.7–6.6%. Its overall evidence did not justify runtime dispatch, unsafe intrinsics, and a second backend. The AArch64 NEON candidate was written but neither compiled nor run on this x86-64 host; both SIMD backends were removed from final code. The [kernel archive](../../../bench/results/index-construction-data-path/2026-11-12-ab616dc6aa/ascii-folding-kernel/README.md) holds all 48 role/case rows per trial and the binary hash.

The compact branch `task/index-construction-data-path/compact-contributions` at `ee5e176416` replaced per-gram vectors with a shared linked block arena; opt-in profiling was added at pushed `78c290475a`. `GramBlock` remained 16 bytes; its `next` offset occupied prior padding, while `GramColumn` was 8 bytes plus its 8-byte key. In the focused 10,000-record debug fixture, DHAT gross allocations fell by 438 blocks/15,954 bytes for ASCII and 592 blocks/194,770 bytes for Unicode, but process peak live bytes rose by 119,774 and 125,950. Full-corpus peak/post-build RSS decreased materially, but full builds slowed by 34–39%. The retained-memory benefit did not offset that cost on this workload, so the arena is excluded. [Raw DHAT files](../../../bench/results/index-construction-data-path/2026-11-12-ab616dc6aa/allocations/README.md) are process-wide and are not normal-build timing runs.

Matching opt-in 16-worker full-corpus profiles measured B and compact contribution element capacities separately from the final persistent index. B's `size_of` block/descriptor/column store/contribution was 16/24/48/104 bytes; compact's was 16/8/72/152 bytes. Both had 6,807,376 final global blocks and 25,477 columns in ASCII, and 7,248,099 blocks and 37,909 columns in Unicode. The final global element-capacity lower bound was **175,316,640 → 209,148,240 bytes ASCII** and **186,578,224 → 222,791,568 bytes Unicode** (B → compact). Peak admitted batch element bytes were 821,824 → 798,720 ASCII and 988,736 → 798,720 Unicode; peak received-ready queue element bytes were 18,560,768 → 19,223,040 ASCII and 23,156,096 → 19,554,816 Unicode. Process HWM RSS was 543,432,704 → 467,976,192 bytes ASCII and 546,750,464 → 473,931,776 Unicode. The flat arena therefore used more measured element capacity despite lower RSS; separate vector allocations and uncounted allocator/hash overhead prevent deriving RSS from those capacities. Counters exclude active worker contributions, unread channel results, hash controls, and allocator rounding, and ready-queue and global peaks are not simultaneous. These are labeled lower bounds, not complete peak-live totals. Final index bytes remained identical. The [instrumented profiles and B patch](../../../bench/results/index-construction-data-path/2026-11-12-ab616dc6aa/contribution-storage/README.md) are separate from normal timing.

The parallel branch `task/index-construction-data-path/parallel-admission` at `3cecb58840` used disjoint Rayon merge groups after ordered record admission and kept deterministic error selection. An opt-in four-worker profile recorded 293 parallel batches and 24.878 ms of overlapping group intervals, establishing real overlap rather than inferred CPU activity. Those service intervals are not wall time or CPU time. A focused one-worker ASCII comparison was +0.9%; a four-worker ungated comparison was +9.2%. Gating four workers made that comparison −0.4%, but eight workers still regressed +7.1%; one 16-worker check improved −1.7%. The final code excludes this scheduling path after its combined interaction check below.

## Combined interaction and final retained result

The temporary scalar-plus-parallel integration result (`382ba91251`) improved Unicode versus B by a paired median 57.044 ms (4.681%) in three pairs. Its first ASCII three-pair result was mixed (+8.134 ms median); an additional five-pair ASCII run improved by 27.743 ms (2.491%) median. Directly comparing that combined result against scalar-only isolated parallel admission's contribution: ASCII improved by 13.610 ms (1.223%) median, with one regression, while all three Unicode pairs regressed by 18.041, 41.409, and 29.365 ms (29.365 ms / 2.509% median). Given that negative interaction, the worker-count regressions, and the extra complexity, parallel admission was removed from final code in `4f51c45e22`. The published experiment branch and timing evidence remain available.

The final scalar-only integration revision at the time of measurement was `4f51c45e22`. ASCII's initial three pairs were close to neutral (−3.766 ms median), so only that comparison was extended to five pairs. The following are full `build_ns` values in milliseconds from the final measured runs:

| Alphabet | Pair | B build | Final build | Final − B |
| --- | ---: | ---: | ---: | ---: |
| ASCII | 1 | 1,130.448 | 1,102.452 | −27.996 |
| ASCII | 2 | 1,095.011 | 1,106.759 | +11.748 |
| ASCII | 3 | 1,173.854 | 1,068.644 | −105.210 |
| ASCII | 4 | 1,144.346 | 1,070.035 | −74.311 |
| ASCII | 5 | 1,192.182 | 1,072.870 | −119.312 |
| Unicode | 1 | 1,222.690 | 1,187.948 | −34.742 |
| Unicode | 2 | 1,186.702 | 1,149.098 | −37.604 |
| Unicode | 3 | 1,243.634 | 1,233.170 | −10.464 |

| Alphabet | B median build | Final median build | Paired median | Median peak RSS B → final | Median post-build RSS B → final |
| --- | ---: | ---: | ---: | ---: | ---: |
| ASCII, five pairs | 1,144.346 ms | 1,072.870 ms | **−74.311 ms (−6.494%)** | 593.4 → 597.3 MiB | 576.1 → 555.7 MiB |
| Unicode, three pairs | 1,222.690 ms | 1,187.948 ms | **−34.742 ms (−2.841%)** | 599.9 → 601.3 MiB | 578.9 → 570.6 MiB |

The final peak RSS differences are small relative to run-to-run variation; this is not evidence of a scalar-path memory saving. Separate opt-in B/final profiles recorded extraction/index intervals of 686.005 → 609.718 ms ASCII and 736.233 → 663.414 ms Unicode. Admission wall intervals were 554.771 → 536.643 ms ASCII and 581.745 → 545.871 ms Unicode; bigram service durations were 182.802 → 179.773 ms ASCII and 202.071 → 198.227 ms Unicode; posting service durations were 251.859 → 243.944 ms ASCII and 254.710 → 239.336 ms Unicode. These are separate profiled builds, not the ordinary timing trials, and adjacent pipeline measures can overlap. Only the folded-stream path changed in final code, so other profile differences should not be attributed to it without further evidence.

## Correctness and validation

The final code's differential tests compare the direct scalar stream and both alphabets' gram windows against the locked full-fold oracle for every ASCII byte, controls, punctuation, Unicode expansions, deterministic mixed samples, and lengths around 15/16/17, 31/32/33 and 63/64/65. Existing bigram tests cover multiple segments, 63/64/65-slot boundaries, fallback/custom indexes, overflow, and candidate conservatism. The full workspace suite includes repository lifecycle, compaction, mutation, and restore tests.

`cargo test --workspace --locked` passed: 816 tests passed, zero failed, 18 ignored across 21 groups. `cargo clippy --workspace --all-targets --locked` passed with existing warnings. The repository's `scripts/format-rust-spacing.py --check` did not pass because of widespread pre-existing formatting drift; plain `cargo fmt --all -- --check` also fails at B. Targeted `rustfmt --check` on changed bigram files still reports only unchanged, previously formatted assertions from the baseline test file; `git diff --check` passes. The formatting logs are archived; no unrelated formatting cleanup was made.

Baseline and final builds had identical results for all 194 extended Musenalm queries in ASCII and Unicode when hashed over exact count and ordered `SearchHit` values, which include record ID, source path/range, status, diagnostics, matched fields, and payload. Q001, Q108, Q116, Q120 and Q194 matched individually. All four baseline/final snapshots restored under the final code in `SnapshotMode::Required` with `restored=true` and zero build parses, without rebuilding. Snapshot payloads were 129,139,458 bytes ASCII and 131,650,034 bytes Unicode for both roles. Snapshot byte hashes differed because serialization order is not the contract; logical index counters and query results matched. Both roles had zero failed parses, 300 posting fields, eight sort fields, 228,503 relation edges, and 15 bigram stores. Estimated final index bytes matched: 69,662,968 ASCII and 72,571,368 Unicode; distinct gram keys were 25,477 ASCII and 37,909 Unicode. No dependency or persistent index change was made.

The reported speedups are warm-cache results on this host and corpus, not a predicted gain on every collection. The AArch64 SIMD backend was not executed and is absent from final code. The compact and parallel candidates remain in their pushed experiment branches for review; neither is part of the final persistent index.
