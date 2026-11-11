# Structural predicate index repair: Musenalm results

Task: `index-array-length-repair`. Measured on 2026-11-11 from base `95f1736d448a01d94650afd237149c158c0e7a8a` to implementation `90b52907c7f9c9364305e38e1b689c9766cc84d0` on `master`. The corpus and benchmark queries/configuration were unchanged. Both full-suite manifests record corpus fingerprint `db869a76f26e1a3275a5162e5a207f47fb50fa8be02c4bdcfe3f1db50cfb747c` and workload fingerprint `01d846ea70d4fb3e4c4a98c28f4a48400b9505f0d4e21bcaa4251dbcff52e7ec`.

The machine used Linux 7.2.13-200.fc44.x86_64, AMD Ryzen 7 5700G (16 logical CPUs), and rustc 1.99.1. The baseline binary was built from the archived base revision before editing source. The final binary was rebuilt after the implementation commit. Results are under [`bench/results/structural-predicate-index-repair/2026-11-11/`](../../../bench/results/structural-predicate-index-repair/2026-11-11/); `baseline-*` and `final-*` manifests identify the actual revisions. Each benchmark performed an explicit index build with normal Musenalm registrations. The five focused queries used Unicode, seven repetitions after warm-up, four query workers, and **zero verified-result-cache bytes**. Medians and ranges below are observed on this computer; they are not latency guarantees.

## Five required queries

The normal benchmark settings were retained: `count=true`, `evidence=true`, locator output, and the original limits. The table shows min / median / max query time in milliseconds; each case had seven samples. The candidate count is the planner's selected candidate domain. All five returned the same count and first page on both revisions.

| Case and predicate | Matches | Base time, ms | Final time, ms | Median speedup | Candidates, base → final | Source opens / parses / scans, base → final | Source bytes, base → final |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Q115 `items:length >= 2` | 244 | 18.192 / **19.395** / 27.478 | 0.100 / **0.108** / 0.271 | 178.9× | 4,928 → 244 | 4,928 / 4,928 / 4,928 → **0 / 0 / 0** | 11,217,720 → **0** |
| Q116 `contributors:length > 0` | 70,428 | 442.730 / **472.949** / 533.772 | 8.339 / **10.051** / 11.443 | 47.1× | 133,948 → 70,428 | 133,948 / 133,948 / 133,948 → **0 / 0 / 0** | 149,889,026 → **0** |
| Q117 `contributors:length = 0` | 63,520 | 404.834 / **429.785** / 435.559 | 9.139 / **18.109** / 20.875 | 23.7× | 133,948 → 63,520 | 133,948 / 133,948 / 133,948 → **0 / 0 / 0** | 149,889,026 → **0** |
| Q118 `contributors:length >= 3` | 254 | 397.679 / **402.704** / 412.251 | 0.687 / **0.699** / 0.822 | 576.4× | 133,948 → 254 | 133,948 / 133,948 / 133,948 → **0 / 0 / 0** | 149,889,026 → **0** |
| Q120 `incipit_statement = ""` | 42,700 | 369.942 / **403.198** / 404.716 | 9.279 / **9.672** / 11.210 | 41.7× | 133,948 → 42,700 | 133,948 / 133,948 / 133,948 → **0 / 0 / 0** | 149,889,026 → **0** |

The optimized first-50 pages and complete counts were compared with `Repository::search_reference` over the full corpus, separately in both revisions. The example [`verify_structural_reference.rs`](../../../crates/core/examples/verify_structural_reference.rs) fails if optimized and reference hit membership differs. The ordered first-page identity hashes also match across revisions:

| Case | Count | Ordered first-page SHA-256, both revisions |
| --- | ---: | --- |
| Q115 | 244 | `941c1ac7f8fcebcf575c7f566618cab158564c8898611c72583834b2d3e2a6c0` |
| Q116 | 70,428 | `685b5ef8584546a358667e7d29a3d69ba387e79a809cc67344f88d37756ad800` |
| Q117 | 63,520 | `99fa3c497d8c069e21d4c183f2c9863a0fd2fa4add4ade421ef82ed0d2dda2cf` |
| Q118 | 254 | `dc317480648c21b224ebf47d8ac16bdc41ddba43720fe1cf83ba251f50874d3d` |
| Q120 | 42,700 | `d524fd26bbf60d419b0378c225a1d1e35c1dec53290b8ec1123d6f9903fc50e5` |

Raw verifier outputs are [`reference-baseline.log`](../../../bench/results/structural-predicate-index-repair/2026-11-11/reference-baseline.log) and [`reference-final.log`](../../../bench/results/structural-predicate-index-repair/2026-11-11/reference-final.log). The hashes cover the ordered page identities; the reference comparison also checks the complete count, not an exhaustive list of all matching identities.

## Complete suite and unrelated timings

The existing 194-query suite ran in ASCII and Unicode, each cache-off and warm, with three repetitions: **776 of 776 rows passed** on the final revision. All 776 `total_matches` and `returned_records` values equal baseline. In both cache-off profiles, Q080, Q108, Q115, Q116, Q117, Q118, Q120, and Q122 changed from source-reading paths to zero source opens, bytes, parses, and scans. Warm verified-result hits are excluded from this source-work claim.

The three-sample suite showed 17 ASCII-off and 9 Unicode-off rows with both >5% and >0.5 ms higher medians. Their source-work and candidate counters did not increase. Paired seven-sample controls and reverse-order eleven-sample controls showed that several medians shift with run conditions, with unchanged plans and work counters. For example, the final suite's Q076 ASCII median was 33.084 → 41.071 ms; a paired seven-sample run measured 34.974 → 33.414 ms with the same source scan. Q048 ASCII was 0.247 → 0.852 ms in the full suite, but 0.217 → 0.220 ms in the paired run. These full-suite increases do not show a repeatable additional source-work path.

Q135 remains a timing limitation: its ASCII-off full-suite median was 13.213 → 20.267 ms and a paired seven-sample run was 13.971 → 18.463 ms. Those paired ranges overlap substantially (11.812–24.443 ms base; 11.940–20.813 ms final), while Unicode medians were 17.132 → 17.230 ms in the same paired run. Both revisions visited 133,948 candidates, opened no source files, and retained the same sort-key byte count and indexed sort path. The measured ASCII median cost is real for these runs, but the counters and overlapping distribution do not identify a structural-index operation as its cause. No sort-path rewrite is included in this repair.

Q119 (`@file.modified >= "2026-01-01T00:00:00Z"`) already has zero source work. The final full-suite Unicode-off median was 26.748 → 27.294 ms; ASCII-off was 26.363 → 26.898 ms. The exact metadata path still visits 133,948 candidates and spends about 7.3 ms in planning; the remaining time is metadata membership and result/count/page processing. No redundant local operation was established. The separate follow-up is [`q119_metadata_result_processing.md`](../issues/perf/q119_metadata_result_processing.md).

## Index construction and storage cost

Full-suite build measurements below are from one explicit build per alphabet, so small timing differences should be interpreted cautiously. Estimated index bytes and snapshot bytes rose by about 1%. Peak RSS was below the requested 5% investigation threshold in these runs.

| Alphabet | Build, base → final | Peak RSS bytes, base → final | Estimated index bytes, base → final | Snapshot bytes, base → final | Snapshot write, base → final |
| --- | ---: | ---: | ---: | ---: | ---: |
| ASCII | 2.508 → 2.529 s (+0.83%) | 565,739,520 → 577,744,896 (+2.12%) | 68,963,924 → 69,662,968 (+1.01%) | 127,418,088 → 129,139,458 (+1.35%) | 0.411 → 0.438 s |
| Unicode | 2.547 → 2.570 s (+0.89%) | 783,998,976 → 810,258,432 (+3.35%) | 71,872,324 → 72,571,368 (+0.97%) | 129,928,664 → 131,650,034 (+1.32%) | 0.450 → 0.436 s |

The ASCII snapshot-write difference exceeds 5% in this single full-suite build, but a paired control measured 0.431 → 0.431 s and did not reproduce it. The paired controls also showed RSS variation. The stable size increase is the retained structural facts and persisted capability flags, not a second general value dictionary.

## Coverage, correctness, and persistence

Declared permitted arrays retain length postings even when scalar element indexing is omitted, including object arrays. Permitted scalar strings retain exact empty-string coverage without storing every nonempty value. Fully indexed fields reuse existing postings. An explicit `index: false` remains an opt-out. The opt-in schema-free auto-index path retains these facts under its existing budgets; inference remains opt-in. A field without complete scalar-value coverage cannot answer arbitrary equality, range, text, or sort predicates from the partial posting. Unsupported nested, repeated, quantified, or explicitly selected array expressions keep the reference/residual evaluator. Missing, null, wrong-type, and empty values remain distinct.

The facts are built during existing parse/build and explicit add/update/rebuild, with deletion, movement, segmented compaction, and save/load covered by tests. Query-time source freshness behavior is unchanged. Snapshot formats advanced from 19/20 to 21/22; prior snapshots are rejected by the compatibility check and require an explicit rebuild before these facts are available.

`cargo test --workspace --locked` passed after the implementation commit, including core differential, lifecycle, storage, CLI, and desktop tests. Task-owned Rust files passed `rustfmt --edition 2024 --check`, and `git diff --check` passed. No benchmark configuration or query text was changed.

## Reproduction and raw data

From the repository root, build the example at each revision with `cargo build --release --locked --example bench_musenalm`. Run the same binary with `--root examples/musenalm --extended --profiles ascii-off,unicode-off,ascii-warm,unicode-warm --repetitions 3 --query-workers 4 --verified-cache-bytes 67108864 --result-dir <directory>` for each full suite. For focused runs, use `--cases Q115,Q116,Q117,Q118,Q120 --profiles unicode-off --repetitions 7 --query-workers 4 --verified-cache-bytes 0`. Run `cargo run --release --locked --example verify_structural_reference -- examples/musenalm` at each revision to compare optimized and reference results; the example was added on the final revision and built against the archived baseline source for the baseline verifier output.

The committed result directories contain CSV summaries, individual measurements, query plans, index results, and manifests. Paired control directories are also retained. Their benchmark manifest `code_revision` is the process working directory's revision, which can be misleading for isolated archived binaries: `control-baseline`, `reverse-baseline`, and `control2-baseline` executed the base binary; `control-repaired` and `reverse-repaired` executed a precommit build of the final source; `control2-final` executed the committed final binary. Use `baseline-*` and `final-*` for the exact-revision primary comparison.
