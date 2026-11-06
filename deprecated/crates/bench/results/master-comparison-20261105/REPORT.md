# Current branch versus master: final query and index comparison

## Inputs and method

The reference is local `master` at `2d6d876ee7f308890ee19868d18ad26ebea31cc3`. The measured current branch is `library-extensions-stage2-wip` at `90b8db3c7e67d5102355e7525da90a8733ce890b`. The master checkout was clean except for the saved [benchmark-only driver patch](master-benchmark-harness.patch), which makes `index-build-profile` use the normal allocator unless `allocation-profile` is selected. The core production code remained exactly at master. The JSON query driver and 152-case workload are byte-identical across the revisions.

Both binaries read the same source paths under the current checkout. The [before](corpora-before.json) and [after](corpora-after.json) content-and-stat fingerprints are byte-identical and match the previously recorded five-corpus fingerprint. The measured host used Rust/Cargo 1.99.1, `-C target-cpu=native`, 16 Rayon threads and CPUs 0–15. Timed processes were serialized. Filesystem caches were warm; no physical-storage cache eviction was attempted. Exact revisions, executable hashes, lockfile hashes, settings and source identities are in the [manifest](manifest.json). Executables and revision-specific snapshots remain local under `target/benchmarks/master-final-20261105/` and are excluded from Git.

The binaries were built with these commands, with `CARGO_TARGET_DIR=/home/simon/source/objekte/target` for the master worktree. Each executable was copied before the next build could overwrite it:

```sh
cargo build --locked --release -p objekte-bench --bin json-query-mix -j 2
cargo build --locked --release -p objekte-bench --features index-timings --bin index-build-profile -j 2
```

The paired query run was:

```sh
python3 crates/bench/run_paired_library_matrix.py \
  --reference target/benchmarks/master-final-20261105/bin/master \
  --candidate target/benchmarks/master-final-20261105/bin/current \
  --expected-input crates/bench/results/library-extension-candidate/final-matrix/corpora-post-reorg.json \
  --actual-input target/benchmarks/master-final-20261105/corpora-before.json \
  --output target/benchmarks/master-final-20261105/queries \
  --pairs 1 --rounds 7 \
  --only json-schema-ascii-cache0,json-schema-ascii-cache64
```

The runner pins `RAYON_NUM_THREADS=16` and `taskset -c 0-15`. It ran the ordinary ASCII, configured-schema, `Bigram` backend at 0 and 64 MiB query-cache budgets. Each invocation warmed indexes and queries before its seven measured rounds. The [raw query rows and commands](raw/queries/) preserve all samples, setup fields and stderr. The query benchmark records reported totals (capped at 10,000), first result IDs and source-work counters; it does not record complete result pages.

Index creation and restoration used the same [paired profile runner](run_profiles.py) and existing ten profile definitions:

```sh
python3 crates/bench/results/master-comparison-20261105/run_profiles.py \
  --master-bin target/benchmarks/master-final-20261105/bin/master/index-build-profile \
  --current-bin target/benchmarks/master-final-20261105/bin/current/index-build-profile \
  --output-dir target/benchmarks/master-final-20261105/index --runs 5
```

Each build sample is a fresh `FilePicker::profile_collection` process with `--skip-snapshot`. The runner prepared separate revision-specific snapshots before timing, then made five alternating build and five alternating restore samples for every profile. The build feature set was `index-timings` without `allocation-profile`, so both used the normal allocator with identical stage timers. Index creation does not include subsequent snapshot writing. Restoration is complete base-plus-secondary restore from its own revision's snapshot. Combined [build](raw/index/build.ndjson) and [restore](raw/index/restore.ndjson) rows contain every timed process result; the [runner](run_profiles.py) reconstructs each command. Per-process stdout, stderr and command records are preserved locally under `target/benchmarks/master-final-20261105/index/raw/`.

## Query results and timing

All 152 reported totals and first IDs matched at both budgets. Every recorded per-case scan, source-read, parse and cache-hit counter matched as well. The complete [152-row table](query-table.md) gives the query text, reported total, first ID, each revision's seven-round median and absolute/relative delta. A reported total of 10,000 may be capped, so it is not necessarily the actual full match count.

| Query cache | Sum of master case medians | Sum of current case medians | Difference |
| --- | ---: | ---: | ---: |
| 0 MiB | 11,095.610 ms | 10,825.869 ms | −269.741 ms (−2.43%) |
| 64 MiB | 10,050.922 ms | 9,867.375 ms | −183.547 ms (−1.83%) |

These sums are a compact view of per-case medians, not one end-to-end workload timing. In the initial 64 MiB pair, several short cases appeared 7–10% slower, generally by less than 1 ms. An 11-case focused workload was then run in three alternating pairs of seven rounds through the [repeat runner](repeat_query_cases.py), with [raw outputs](raw/query-repeats/). Representative median paired deltas were `annotation ~ :ignorecase('Mond')` +0.055 ms, `preferred_title = 'Der Mond'` +0.142 ms, `musenalm_type ?= 'Gedicht/Lied'` +0.177 ms and `contributors.type ?= 'Autor:in'` +0.154 ms. `owner = 'Theodor Springmann Stiftung' && basis = true` changed from +1.538 ms in the initial full run to −0.107 ms in the focused repeats. All repeated signatures matched. The smaller, repeatable changes should not be read as a broad 7–10% regression; the focused workload has different cache interactions from the full workload.

## Index creation

All ten profiles had identical document counts, compressed index bytes and index-size maps across revisions and samples. Negative deltas favor the current branch. Medians are from five fresh processes per revision; raw samples and min/max spreads are in [summary.json](summary.json).

| Profile | Records | Master median (ms) | Current median (ms) | Δ ms | Δ % |
| --- | ---: | ---: | ---: | ---: | ---: |
| amazon-base | 1,000,000 | 1,895.175 | 1,853.382 | −41.793 | −2.21% |
| amazon-fts2 | 1,000,000 | 2,868.979 | 2,878.675 | +9.696 | +0.34% |
| leipzig-base | 400,000 | 951.181 | 923.308 | −27.873 | −2.93% |
| leipzig-fts1 | 400,000 | 1,219.635 | 1,212.663 | −6.972 | −0.57% |
| lenz-base | 2,133 | 7.713 | 7.706 | −0.007 | −0.09% |
| lenz-fts2 | 2,133 | 12.194 | 12.312 | +0.118 | +0.97% |
| musenalm-base | 133,948 | 1,195.347 | 1,185.953 | −9.393 | −0.79% |
| musenalm-fts4 | 133,948 | 1,527.605 | 1,516.761 | −10.845 | −0.71% |
| nyc-base | 1,000,000 | 2,116.619 | 2,102.381 | −14.238 | −0.67% |
| nyc-fts3 | 1,000,000 | 3,368.231 | 3,339.083 | −29.148 | −0.87% |

## Snapshot restoration

Restored document counts, compressed index bytes, index-size maps and both base and secondary snapshot byte counts matched in every profile. The initial Amazon base (+26.710 ms) and Musenalm FTS4 (+13.897 ms) median differences were checked with five additional alternating pairs using the same prepared snapshots. Amazon base then differed by only +0.397 ms. Musenalm FTS4 remained +7.543 ms (+2.23%): its stage medians locate about +6.508 ms in secondary structure decoding and +1.238 ms in manifest decoding. This is a small repeatable restore difference, not a size or completeness difference. The [repeat rows](raw/index/restore-repeats.ndjson) include each exact command.

| Profile | Records | Master median (ms) | Current median (ms) | Δ ms | Δ % |
| --- | ---: | ---: | ---: | ---: | ---: |
| amazon-base | 1,000,000 | 755.952 | 782.661 | +26.710 | +3.53% |
| amazon-fts2 | 1,000,000 | 1,023.757 | 1,015.670 | −8.086 | −0.79% |
| leipzig-base | 400,000 | 280.336 | 283.820 | +3.484 | +1.24% |
| leipzig-fts1 | 400,000 | 324.921 | 324.481 | −0.440 | −0.14% |
| lenz-base | 2,133 | 3.020 | 3.037 | +0.016 | +0.54% |
| lenz-fts2 | 2,133 | 6.028 | 3.862 | −2.165 | −35.92% |
| musenalm-base | 133,948 | 298.821 | 303.005 | +4.185 | +1.40% |
| musenalm-fts4 | 133,948 | 353.072 | 366.969 | +13.897 | +3.94% |
| nyc-base | 1,000,000 | 686.940 | 691.228 | +4.288 | +0.62% |
| nyc-fts3 | 1,000,000 | 972.222 | 959.621 | −12.601 | −1.30% |

Lenz restore samples span about 1.7–12 ms, so its large percentage in one profile does not support a stable speed claim. The saved measurements use warm filesystem caches and timer-instrumented index builds/restores. They do not measure cold physical storage, the full repository open lifecycle, custom-format index construction or complete query result pages.
