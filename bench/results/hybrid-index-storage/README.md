# Hybrid index storage benchmark archive

Task `hybrid-index-storage`. This archive records the pre-implementation baseline and the Phase 0 inference-timer correction. It is **not** a final acceptance result.

## Baseline environment and fixture

- Source revision: `da8fde68472b8646a63a05b3b3654c58de877301` (fetched `origin/master`, with no local changes to the measured executable).
- Rust `1.99.1`; Linux `7.2.15-200.fc44.x86_64`; 13th Gen Intel Core i9-13900K; performance governor; requested 32 build workers, affinity reported as CPUs 0–31.
- Btrfs with 4096-byte blocks on the local NVMe-backed filesystem. No host-wide cache clearing was performed; the repeated restore measurements are warm-page-cache measurements.
- Configured Musenalm: 148,486 files/records, 193,297,230 source bytes; inventory fingerprint `ad517d15e3216bfbdb569906d14a1717271daaf27a8d58e47e55e1e63e5db868`; schema fingerprint `4115070eb30b0af98b109429d0883b493f857cdf671ec55b943d91b73c5ebfe3`.
- ASCII format-28 baseline snapshot: 132,710,084 bytes, SHA-256 `4476f1e0f1f5729da8592ec1f5b7520c52877892a428dc8828afc4979d575d63`. The snapshot is ignored and retained locally at `target/hybrid-index-storage/baseline/fixture/trial-*/index.snapshot`; its metadata, sample, and exact command are archived in `baseline/fixture/`.
- Baseline executable SHA-256: `a39a37dc050aa2a499542661ef5d3a2233314da00f01336696eb330018edb642` for `bench_index_construction`, and `01deffe55c7c7a343d208aeb4453f8b1202b0688945175fd54e1bd851320a048` for `bench_musenalm`. The executables themselves remain ignored under `target/hybrid-index-storage/baseline/`.

The fixture and executable hashes matter: later paired comparisons must run the retained baseline executable and the new executable on comparable data and cache conditions. The task worktree's absolute root differs from older archived Musenalm measurements; their corpus fingerprints are not assumed interchangeable.

## Commands

From the integration worktree root, build the baseline at its recorded revision in a separate worktree or retain the exact binary above:

```sh
cargo build --release --locked -p objekte-search-core --example bench_index_construction --example bench_musenalm
target/release/examples/bench_index_construction --root examples/musenalm --result-dir target/hybrid-index-storage/baseline/fixture --alphabet ascii --workers 32 --incremental --snapshot --check-queries
```

Seven fresh-process builds used `--incremental --alphabet ascii --workers 32` with no `--snapshot` and separate `--result-dir target/hybrid-index-storage/baseline/build`. Seven restore processes used the same arguments plus `--restore-snapshot <fixture/index.snapshot>` and `--result-dir target/hybrid-index-storage/baseline/restore`. Each restore used the **same** fixture and a new process. `baseline/baseline-trials.json` gives execution order and output paths; the trial directories hold raw samples, manifests, and query digests. The build and restore trials alternated by round.

The complete established extended query campaign had 220 cases and ran with result caching disabled:

```sh
target/release/examples/bench_musenalm --root examples/musenalm --result-dir target/hybrid-index-storage/baseline/queries --extended --incremental --profiles ascii-off,unicode-off --verified-cache-bytes 0 --repetitions 5
```

`baseline/queries/` contains every case's five raw timing samples, plan traces, digests, manifest, and report. All cases reported `ok`. Seven build trials had median **1,088.23 ms**, and seven restores had median **678.53 ms**. The query campaign builds were separate from the seven construction trials.

## Inference timer contract

The Phase 0 revision separates collection-wide inference observation from coordinator bookkeeping. After that code change, a release build with `--features detailed-profiling` ran three fresh-process `--incremental --profile` trials with inference disabled and three with `--inference`, otherwise using the same corpus, alphabet and workers. Raw samples are under `profile-contract/`.

| Median over 3 profiled trials | Disabled | Enabled |
| --- | ---: | ---: |
| Total build | 1,037.93 ms | 2,462.69 ms |
| Exclusive admission | 394.39 ms | 1,323.11 ms |
| Collection inference observation | 0.01 ms | 463.78 ms |
| Segment-local inference merge | 0.017 ms | 420.24 ms |
| Coordinator bookkeeping | 6.65 ms | 8.96 ms |

These timings diagnose where inference work occurs. They are not a matched speed comparison with the baseline executable. The disabled mode does not establish enabled-mode cost.
