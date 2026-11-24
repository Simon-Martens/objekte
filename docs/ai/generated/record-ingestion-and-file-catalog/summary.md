# Record ingestion and optional file catalog: final measurement

| Metric / gate | Configuration or QID | Before | After | Delta | Verdict |
| --- | --- | ---: | ---: | ---: | --- |
| Build wall | ASCII | 627.616 ms | 645.574 ms | +17.959 ms (+2.86%) | Pass |
| Build peak RSS | ASCII | 599.929 MB | 644.182 MB | +44.253 MB (+7.38%) | Pass |
| Build wall | Unicode | 706.673 ms | 705.074 ms | −1.599 ms (−0.23%) | Pass |
| Build peak RSS | Unicode | 652.345 MB | 659.038 MB | +6.693 MB (+1.03%) | Pass |
| Focused nine-query median sum | ASCII | 118.168 ms | 121.458 ms | +3.290 ms (+2.78%) | Pass |
| Focused nine-query median sum | Unicode | 118.081 ms | 119.439 ms | +1.358 ms (+1.15%) | Pass |
| Focused individual queries | 18 ASCII/Unicode rows | See CSV | See CSV | Largest: ASCII Q144 +2.090 ms (+3.29%) | Pass, all rows |
| Extended individual queries | 6 Unicode rows | See CSV | See CSV | Largest: Q219 +4.368 µs (+2.76%) | Pass, all rows |
| Direct ID lookup | 16,384-file probe | 149.351 ns/op | 177.078 ns/op | +27.727 ns (+18.57%) | Pass; below 5 µs floor |
| Projected residual query | Year/note probe | 485.862 µs | 446.692 µs | −39.170 µs (−8.06%) | Pass |
| Historical screening confirmation | 20 ASCII + 10 Unicode QIDs | See CSV | See CSV | Largest positive: ASCII Q144 +2.029 ms (+3.06%) | Pass, all 30 QIDs |
| Small file lifecycle confirmation | Add | 1.668 ms | 2.158 ms | +0.490 ms (+29.38%) | **Breach** |
| Small file lifecycle confirmation | Move | 0.054 ms | 0.065 ms | +0.011 ms (+20.37%) | **Breach** |
| Small file lifecycle confirmation | Other six phases | See CSV | See CSV | Largest positive: build +0.224 ms (+4.96%) | Pass |
| Correctness and experimental conditions | All paired workloads | 14/14 passed | 14/14 passed | Same fixture, configuration, lockfile and toolchain | Pass |
| New typed ingestion and routing | 27 after-only runs | N/A — API absent | 27/27 passed | Absolute costs below | Pass |

**Overall verdict: blocked.** The confirmed add and move slowdowns each exceed both the 10% relative and 5 µs absolute limit. Section 10.3 of the task specification makes a confirmed lifecycle breach block completion. The comparison uses the median of three process observations per side, with alternating run order. No exception has been granted. The implementation remains on the integration branch; the task has not been marked done.

## Largest changes and lifecycle detail

The largest passing absolute time increase is ASCII build wall, +17.959 ms (+2.86%). Among focused queries, ASCII Q144 rises +2.090 ms (+3.29%). Among confirmed historical screening cases, ASCII Q144 rises +2.029 ms (+3.06%). The largest passing relative increase with an absolute rise above 5 µs is historical ASCII Q137, +11.974 µs (+6.92%). The ID lookup percentage (+18.57%) is larger, but its absolute difference is 27.727 ns, far below the specified 5 µs floor. ASCII build peak RSS rises +44.253 MB (+7.38%), below the 10% build gate. The largest confirmed lifecycle regression is add, +0.490 ms (+29.38%).

The three interleaved lifecycle process observations were:

| Phase | Before, ms | After, ms | Median delta | Gate |
| --- | --- | --- | --- | --- |
| Build | 4.637, 4.515, 4.351 | 4.739, 4.299, 5.062 | +0.224 ms (+4.96%) | Pass |
| Add | 1.668, 1.766, 0.290 | 1.898, 2.158, 2.159 | +0.490 ms (+29.38%) | **Breach** |
| Update append | 0.168, 0.190, 1.036 | 0.156, 0.325, 0.157 | −0.033 ms (−17.37%) | Pass |
| Update verified | 0.141, 0.207, 0.175 | 0.202, 0.185, 0.180 | +0.010 ms (+5.71%) | Pass |
| Move | 0.052, 0.084, 0.054 | 0.062, 0.076, 0.065 | +0.011 ms (+20.37%) | **Breach** |
| Delete verified | 0.040, 0.065, 0.038 | 0.037, 0.045, 0.048 | +0.005 ms (+12.5%) | Pass; exactly 5 µs, not over |
| Compact | 0, 0.001, 0 | 0.001, 0.001, 0.001 | +0.001 ms; zero baseline | Pass |
| Reconcile no change | 0.175, 0.147, 0.336 | 0.165, 0.159, 0.156 | −0.016 ms (−9.14%) | Pass |

The preceding three-run matrix screened update append (+0.150 ms, +90.36%) for confirmation. It cleared confirmation (−0.033 ms, −17.37%). Add and move were below the screening threshold in the matrix but failed the interleaved confirmation. Earlier candidate measurements and their confirmations remain in the evidence directory to show that the small-file timings vary with process conditions; they do not replace the final-SHA confirmation verdict.

## New API absolute costs

All ingestion runs published exactly 65,536 unique typed records, including one- and 256-record batches, one and four producers, and normal and forced overflow modes. `producer_accept_ns` excludes prebuilding inputs and `publish_drain_ns` measures completion after admission. Values below are medians of three fresh runs; the base has no corresponding API.

| Configuration | Producer acceptance | Publish/drain | RAM high water | Spool high water | Spool written |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 record, 1 producer, normal | 14.459 ms | 30.367 s | 32.563 MB | 0 | 0 |
| 1 record, 4 producers, normal | 30.156 ms | 31.817 s | 32.514 MB | 0 | 0 |
| 256 records, 1 producer, normal | 0.253 ms | 176.768 ms | 24.379 MB | 0 | 0 |
| 256 records, 4 producers, normal | 0.356 ms | 172.495 ms | 24.284 MB | 0 | 0 |
| 256 records, 1 producer, overflow | 18.806 ms | 191.547 ms | 1.048 MB | 6.637 MB | 6.637 MB |
| 256 records, 4 producers, overflow | 37.096 ms | 197.093 ms | 1.048 MB | 6.637 MB | 6.637 MB |

Point-route tests used 65,536 records grouped 64 per linked file and one, eight or 64 segments. Each of three fresh runs per segment count passed 10,000 alternating hit/miss lookups, enumerated all 65,536 reverse members, and visited 5,000 target segments for the 5,000 hits, with no unrelated segment visits. Median lookup times were 1.586 ms (one segment), 1.575 ms (eight), and 1.635 ms (64) per 10,000 operations. Empty and unknown file behavior passed. Exact per-run timing, memory, bytes and segment metrics are in `after/native/manifest.json`, with medians in `comparison.csv`.

## Correctness, conditions and evidence

The measured base is `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`; the final production implementation is `55f6bb8ba596f1503d232f86a67a58a233c441b4`. No production code changed after the final measurements. Both sides used Rust/Cargo 1.99.1, default release allocator and flags, CPU affinity 0–31, the performance governor, 32 build workers and one query worker. The canonical read-only fixture `/home/simon/source/objekte/examples/musenalm` had 148,498 JSON files. Its content SHA-256 was `79acbab9155aa95bc869afde56dc1b402e5a09f9c6a80a5106fc665e2c3f20c5` and metadata SHA-256 was `b0a47239de14ad78c0aa059c3c1a6e48f3d988ae234d6c904fc78c3a96429106` on both sides. The lockfile and workload manifest digests, build flags, exact binary hashes, commands, outputs and statuses are in the side manifests. Separate output paths and fresh mutation fixtures were used for every invocation.

Canonical ordered answers and semantic digests matched byte for byte for ASCII, Unicode and extended Unicode. Both 150-query historical profiles retained status, counts, candidates, source reads and parses. The controlled exact-year count returned 1,024 with zero source fetches. The direct and residual probes returned the expected memberships. The native after-only runs had no failed invocations. `after/validation.json` records these checks; `comparison.csv` contains every paired and after-only row. Raw output, manifests and SHA-256 lists are retained under `before/`, `after/` and `confirmation/`, with earlier candidate sets labeled by their measured SHA.

Validation on the final implementation SHA: `cargo fmt --all -- --check`, `cargo check --workspace --all-targets`, and `cargo test --workspace --quiet` all passed. The workspace tests included 691 active core tests and 12 integration tests. The paired examples were built with `cargo build --locked --release -p objekte-search-core --example bench_build_musenalm --example bench_musenalm --example bench_lifecycle_mutations` from each measured worktree. The after-only native example was built and run from the final worktree. The relevant issue is [`record-ingestion-lifecycle-add-move-confirmation.md`](../../issues/perf/record-ingestion-lifecycle-add-move-confirmation.md).
