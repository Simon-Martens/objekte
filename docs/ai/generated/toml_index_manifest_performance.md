# TOML index manifest performance and equivalence report

Task: `toml-index-manifest`  
Date: 2026-11-14  
Baseline: `a0a0691897c6ff24985fc4db9ae51f5d77a89899` (`master`, explicit index persistence)  
Candidate: `b31db7b2eb97bfe3d5cf9e047a823845b299d647` (`integration/toml-index-manifest`)  
Environment: Linux 7.2.15-200.fc44.x86_64, Intel Core i9-13900K, 32 logical CPUs, Rust 1.99.1, release builds, 32 query workers.

All stores were built from the 148,486-record Musenalm fixture. Baseline and candidate stores have identical logical contents. Stores and raw samples are under the candidate worktree's ignored `target/toml-index-manifest/bench/` directory. Load timing is an uninstrumented `Index::try_load` measured by `bench_managed_open_alloc --no-count`; it includes TOML parsing, directory enumeration, mapping, descriptor restoration, catalog validation and install, and excludes optional whole-file verification.

## Warm-cache explicit load

Thirty alternating fresh-process pairs per scenario after one warm-up per revision. Ratios are per-pair candidate/baseline medians.

| Scenario | Baseline median | Baseline p95 | Candidate median | Candidate p95 | Ratio |
| --- | ---: | ---: | ---: | ---: | ---: |
| ASCII, 8 segments | 238.064 ms | 243.511 ms | 226.668 ms | 233.630 ms | 0.9513 |
| Unicode, 8 segments | 239.903 ms | 248.407 ms | 226.042 ms | 233.283 ms | 0.9420 |
| ASCII, 71 segments after mutations | 245.968 ms | 255.018 ms | 231.140 ms | 239.282 ms | 0.9440 |

All runs reported zero source reads and zero source parses before and after the first representative query. The 2% target and 3% acceptance ceiling were met in every scenario.

## Page-evicted explicit load

Five alternating pairs per scenario after `POSIX_FADV_DONTNEED` on every `.idx` and catalog file; `fincore` reported zero resident index bytes after eviction for every trial. This is page eviction, not a cold-boot claim.

| Scenario | Baseline median | Baseline p95 | Candidate median | Candidate p95 | Ratio |
| --- | ---: | ---: | ---: | ---: | ---: |
| ASCII, 8 segments | 255.948 ms | 282.657 ms | 234.288 ms | 237.598 ms | 0.9154 |
| Unicode, 8 segments | 248.120 ms | 258.489 ms | 243.318 ms | 246.498 ms | 0.9806 |
| ASCII, 71 segments after mutations | 277.389 ms | 288.718 ms | 260.750 ms | 286.651 ms | 0.9569 |

## Save-side tradeoff

`bench_managed_flush` measures the explicit initial save after a completed in-memory build. Three alternating ASCII pairs:

| Operation | Baseline | Candidate | Candidate / baseline |
| --- | ---: | ---: | ---: |
| Initial save median | 322.984 ms | 300.923 ms | 0.932 |
| Unchanged save | 0.849 ms | 0.856 ms | 1.008 |
| Process peak RSS, build plus save median | 692,864 KiB | 702,640 KiB | 1.014 |

Changed save on a loaded single-generation store after updating ten source files:

| Measurement | Baseline | Candidate |
| --- | ---: | ---: |
| Changed save | 236.0 ms | 597.3 ms |
| Segment bytes written | 2,938,848 | 112,404,521 |

The candidate writes every segment file anew by design. Two further mutation/save cycles on the 71-segment fragmented stores wrote 8.3 MB and 10.5 MB per baseline generation versus 117.8 MB and 119.9 MB per candidate generation. After those three retained generations, the baseline store occupied 210 MiB and the candidate store 419 MiB. An unchanged explicit save still creates no generation.

The fragmented-store changed-save time could not be measured reliably: a further update cycle on both revisions fails with a pre-existing mutation/partition re-sealing error (reported separately in `docs/ai/issues/bug/fragmented-loaded-save-identity-mismatch.md`). The baseline at `a0a0691897` fails the same scenario with the same class of sealed-segment identity/reference check, so this is not caused by the TOML manifest change.

## Optional verification

A catalog `[verify]` entry was added to a copy of the candidate ASCII manifest. Three verification-enabled opens took 240.407, 238.104 and 235.447 ms; the default open in the same state took 232.250 ms. The catalog was the only hashed file. Default loading performs no manifest or catalog whole-file hashing.

## Query equivalence on saved/reopened indexes

The full 220-case ASCII and 220-case Unicode campaigns were run in explicit-load mode against each revision's own saved store. All 220 result digests matched in every comparison:

- baseline saved/reopened ASCII versus baseline memory build: 0 differences;
- candidate saved/reopened ASCII versus baseline saved/reopened ASCII: 0 differences;
- candidate saved/reopened Unicode versus baseline saved/reopened Unicode: 0 differences;
- candidate memory build versus candidate saved/reopened ASCII: 0 differences.

Every campaign loaded the saved store source-free (`build_parses = 0`, `source_reads = 0`) before its first query. Representative commands:

```sh
target/release/examples/bench_musenalm \
  --root /home/simon/source/objekte/examples/musenalm \
  --result-dir target/toml-index-manifest/bench/campaign-ascii \
  --extended --incremental --load \
  --storage custom target/toml-index-manifest/bench/candidate-ascii \
  --profiles ascii-off --repetitions 1 --query-workers 32
```

The Unicode run used `candidate-unicode` and `unicode-off`. Equivalence was established at revision `b31db7b2eb`; the baseline runner used the same explicit-load mode added to the benchmark example, compiled against `a0a0691897`.

## Workspace checks

```sh
cargo test --workspace --locked -- --quiet
cargo check --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling
```

All tests and checks passed. One desktop export test initially raced between file creation and writing and failed once, then passed alone and in the complete desktop suite on rerun; it is unrelated to persistence.
