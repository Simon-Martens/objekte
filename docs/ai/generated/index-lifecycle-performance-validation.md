# Index lifecycle performance validation

Initial mutation profiling used `task/index-lifecycle/cache-format`; final validation used `integration/index-lifecycle`. Initial source revision `3f9cb54277a8880f3528e23b69c175e4c3fe8212` on 2026-11-08, rustc 1.99.1 release build, 13th Gen Intel Core i9-13900K (32 logical CPUs). Local warm page cache; three independent source clones. This is engineering profiling, not a cold-disk benchmark.

The initial mutation harness was under ignored `target/lifecycle-perf-harness/{Cargo.toml,src/main.rs}`. A safe repeatable version is now committed as `crates/core/examples/bench_lifecycle_mutations.rs`; it requires `OBJEKTE_BENCH_MUTATE_COPY=1` and a disposable fixture copy. Raw results: `small.tsv`, `small.time`, `large.tsv`, `large.time`, `large-2.tsv`, `large-2.time`, `large-3.tsv`, `large-3.time`. Input fixture copied via `cp -a --reflink=auto` from integration `target/lifecycle-fixture/musenalm`, excluding its preexisting legacy cache files. Each large run used a fresh independent clone of 148,486 files (603 MiB). Checked-in examples were never mutated.

Repeat:

```bash
cd /home/simon/source/objekte-task-index-lifecycle-cache-format
CARGO_TARGET_DIR="$PWD/target" cargo build --release --manifest-path target/lifecycle-perf-harness/Cargo.toml
/usr/bin/time -v target/release/objekte-lifecycle-perf small target/lifecycle-perf-harness/small-fixture > target/lifecycle-perf-harness/small.tsv 2> target/lifecycle-perf-harness/small.time
mkdir -p target/lifecycle-perf-harness/large-fixture-new/.objekte
cp -a --reflink=auto /home/simon/source/objekte-integration-index-lifecycle/target/lifecycle-fixture/musenalm/data target/lifecycle-perf-harness/large-fixture-new/
cp -a /home/simon/source/objekte-integration-index-lifecycle/target/lifecycle-fixture/musenalm/.objekte/collections target/lifecycle-perf-harness/large-fixture-new/.objekte/
/usr/bin/time -v target/release/objekte-lifecycle-perf large target/lifecycle-perf-harness/large-fixture-new > target/lifecycle-perf-harness/large-new.tsv 2> target/lifecycle-perf-harness/large-new.time
```

Large medians (three runs; ms; wall time around each API):

| Phase | Median | Range | Observed strategy/result |
| --- | ---: | ---: | --- |
| Fresh build + count | 1311.553 | 1305.540–1380.615 | 148,486 files, 133,948 contents |
| Add one contents file | 0.616 | 0.293–0.624 | append replacement |
| Update one file, append | 0.329 | 0.254–0.336 | append replacement |
| Update same file, verified old bytes | 0.415 | 0.319–0.463 | verified slot preserving, no fallback |
| Move added file | 0.245 | 0.224–0.287 | append replacement |
| Verified delete moved file | 0.139 | 0.120–0.207 | verified physical removal, no fallback |
| Compact | 1246.108 | 1241.096–1253.842 | 1 retired slot removed; 148,486 slots after |
| Flush lifecycle cache | 350.792 | 316.280–444.835 | 140,453,061 bytes written |
| Restore to first count result | 701.015 | 693.305–713.397 | 133,948 contents, background reconcile already running |
| Wait for no-change reconciliation | 878.795 | 860.934–883.750 | 148,486 examined/unchanged; zero issues |

Whole-process peak RSS (`/usr/bin/time -v`): 1,084,812–1,149,180 KiB. Phase HWM rose from about 543–566 MiB after build to about 947–992 MiB after compaction and about 1.03–1.10 GiB by flush. The restore timing includes a count query and concurrent reconciliation, not isolated decode time. No-change reconcile timing is the wait after the immediate searchable result. The flush wall time exceeds the serializer stats (173–293 ms across runs) because it includes guard/preparation work. `VmRSS`/`VmHWM` phase observations are approximate.

Small JSONL fixture (one file, three records before mutation): build 2.011 ms; add 0.134; append update 0.077; verified update 0.127 (physical removal); move 0.149; verified delete 0.031 (physical removal); compact 1.538; flush 1.245; restore to count 0.657; no-change reconciliation 0.046. Restored count 3; no issues.

Existing query baseline manifests: `crates/core/benches/musenalm_queries.json` has 150 cases (SHA-256 `56f2ba99e3a889b95975a6b460c4ef894732ad769f99e6f69086a3f0d3ac91c0`); extended manifest has 194 cases (SHA-256 `edd9ace84c1df94692e4c626d942d334b4fae0731f258477267be97d3ed967b2`). The final full suite below supersedes the interim query plan.

## Final release query comparison, 2026-11-08

Raw files are ignored under `/home/simon/source/objekte-integration-index-lifecycle/target/lifecycle-final/`: `static/` (established runner), `lifecycle-query.jsonl` (pre-fix lifecycle view), `lifecycle-query-after.jsonl` (targeted post-fix), `static-targeted/`, and `*.stdout`/`*.stderr` containing `/usr/bin/time -v` output. The lifecycle runner source is `lifecycle-query/{Cargo.toml,src/main.rs}`. It uses the established extended manifest and the same Musenalm schema registration helper. The source fixture was copied with `cp -a --reflink=auto` to `target/lifecycle-final/fixture`; neither the checked-in example nor the baseline fixture was changed. The copied corpus, schema, and workload hashes exactly match the recorded baseline (`027d5fa1…`, `f94dd058…`, `ea487fe0…`). It contains 148,486 files and records. The page cache was warm; these are release build timings on one i9-13900K, with 32 query workers and five repetitions per case/profile.

Commands (from the integration worktree):

```bash
cargo build --release -p objekte-search-core --example bench_musenalm
/usr/bin/time -v target/release/examples/bench_musenalm --root target/lifecycle-final/fixture --extended --profiles ascii-off,ascii-warm,unicode-off,unicode-warm --repetitions 5 --result-dir target/lifecycle-final/static > target/lifecycle-final/static.stdout 2> target/lifecycle-final/static.stderr
cargo build --release --manifest-path target/lifecycle-final/lifecycle-query/Cargo.toml
/usr/bin/time -v target/lifecycle-final/lifecycle-query/target/release/objekte-lifecycle-final-query target/lifecycle-final/fixture target/lifecycle-final/lifecycle-query.jsonl > target/lifecycle-final/lifecycle-query.stdout 2> target/lifecycle-final/lifecycle-query.stderr
LIFECYCLE_CASES=Q172,Q097,Q129 /usr/bin/time -v target/lifecycle-final/lifecycle-query/target/release/objekte-lifecycle-final-query target/lifecycle-final/fixture target/lifecycle-final/lifecycle-query-after.jsonl > target/lifecycle-final/lifecycle-query-after.stdout 2> target/lifecycle-final/lifecycle-query-after.stderr
```

Established static runner: 194 cases × 4 profiles × 5 repetitions = 3,880 successful samples, zero failures. Baseline `b10f9e9a9b` has the same 3,880 sample matrix and zero failures. The final static executable was compiled around integration `f3472fce25`; the run-manifest's `code_revision` says `118950f42c` because it reads Git HEAD at execution, after the binary was built. Changes between those commits did not modify static query code. The lifecycle full runner was built on integration `3f478a0bf9`, before the bitmap fast-path fix. It completed 194 cases × ASCII/Unicode off × 5 = 1,940 successful samples, with all 388 case/profile exact counts and returned row counts equal to static. Its no-verified-cache profiles are comparable to static `ascii-off`/`unicode-off`; lifecycle warm profiles were not measured because the lifecycle API does not expose the static benchmark's verified-result-cache configuration.

| Profile | Baseline median case ms | Final static median case ms | Median case ratio | 95th percentile case ratio | Ratios >2× |
| --- | ---: | ---: | ---: | ---: | ---: |
| ASCII off | 0.767 | 0.779 | 1.004 | 1.374 | 0/194 |
| ASCII warm | 0.0054 | 0.0055 | 1.012 | 1.860 | 3/194 |
| Unicode off | 0.731 | 0.704 | 1.005 | 1.186 | 1/194 |
| Unicode warm | 0.0056 | 0.0059 | 1.034 | 1.934 | 9/194 |

Ratios are per-case five-sample medians against baseline, then summarized across cases. The >2× warm cases have absolute medians in microseconds. Static Q119 Unicode off rose from 7.47 to 12.02 ms, while ASCII off rose from 7.14 to 7.85 ms and lifecycle Unicode off was 8.67 ms, indicating run variance rather than a consistent route regression. Static Q062 Unicode off rose 0.041→0.197 ms while ASCII off improved 0.187→0.132 ms. The established runner's peak RSS was 891,628 KiB at baseline and 887,204 KiB in the final run. ASCII/Unicode fresh open was 1,164/1,157 ms at baseline and 1,301/1,249 ms in the final run; snapshots grew from 117.3/119.8 MB to 127.4/129.9 MB with lifecycle format changes. A single run per revision does not establish a build-speed regression distribution.

The initial lifecycle view's median per-case timing ratio to the final static runner was 0.992 (ASCII off) and 1.009 (Unicode off); corresponding 95th percentile ratios were 1.772 and 1.869. Q172 was a clear exception: `contents`, `entry = '02i35e3nbm7enwp' && contributors.agents.name ?~ 'Roller'`, six matches and 11 target reads. Static was 0.542/0.564 ms (ASCII/Unicode); lifecycle was 1.948/2.029 ms over five stable repetitions. Its planning counter increased from about 0.66 to 1.94–2.02 ms while relation and source-work counters matched. Q097, an empty `contents` query, rose from 0.437/0.490 to 0.864/0.935 ms. The cause was reconstructing a full active-record bitmap by iterating 148,486 slots for every lifecycle query.

Integration `c83b78a647` makes `ActiveRecords::bitmap()` construct an all-live domain with `insert_range`. A targeted release rerun on that commit (three cases × two profiles × five repetitions, all successful) gave:

| Case | Static targeted ASCII/Unicode ms | Lifecycle before fix ms | Lifecycle after fix ms | Exact count |
| --- | ---: | ---: | ---: | ---: |
| Q172 relation target | 0.558 / 0.558 | 1.948 / 2.029 | 0.092 / 0.100 | 6 |
| Q097 empty result | 0.513 / 0.487 | 0.864 / 0.935 | 0.0022 / 0.0023 | 0 |
| Q129 sorted offset 50 | 0.388 / 0.378 | 0.375 / 0.379 | 0.309 / 0.279 | 3,257 |

Targeted and full-suite cache conditions differ, so the before/after ratios are diagnostic rather than a new overall benchmark. The four pagination cases Q129/Q130/Q135/Q136 were near static parity before the fix (0.95–1.12× across ASCII/Unicode). The targeted fix preserved counts and reduced Q172 planning median to 0.085/0.092 ms. The post-fix lifecycle process peak RSS was 808,840 KiB; the pre-fix full lifecycle process peak was 779,552 KiB. These peaks have different workloads and are not directly comparable.

## Final lifecycle mutation rerun

The committed release runner was built on integration `5052562711` (including the fresh-base digest and full-domain fixes), with rustc 1.99.1, the same 148,486-file Musenalm corpus, and three independent `cp -a --reflink=auto` copies. Each copy retained source metadata; only the disposable copy was mutated. The warm page cache and local i9-13900K match the earlier engineering run. Raw TSV/time logs are under the retained integration worktree at `target/lifecycle-final/mutation-final-{1,2,3}.{tsv,time}`.

```bash
cd /home/simon/source/objekte-integration-index-lifecycle
cargo build --release -p objekte-search-core --example bench_lifecycle_mutations
cp -a --reflink=auto target/lifecycle-final/fixture target/lifecycle-final/mutation-final-1
/usr/bin/time -v env OBJEKTE_BENCH_MUTATE_COPY=1 target/release/examples/bench_lifecycle_mutations large target/lifecycle-final/mutation-final-1 > target/lifecycle-final/mutation-final-1.tsv 2> target/lifecycle-final/mutation-final-1.time
```

Repeat with distinct `mutation-final-2` and `mutation-final-3` copies. The runner refuses to mutate without the explicit environment variable; `small` also requires a nonexistent root.

| Phase | Median ms | Range ms | Initial median ms |
| --- | ---: | ---: | ---: |
| Fresh build + count | 1,284.193 | 1,277.547–1,334.520 | 1,311.553 |
| Add one file | 0.302 | 0.283–0.313 | 0.616 |
| Append update | 0.231 | 0.231–0.245 | 0.329 |
| Verified update | 0.377 | 0.344–0.433 | 0.415 |
| Move | 0.215 | 0.202–0.221 | 0.245 |
| Verified delete | 0.164 | 0.162–0.201 | 0.139 |
| Source-free compact | 1,232.171 | 1,209.559–1,296.686 | 1,246.108 |
| Flush cache | 357.149 | 338.131–366.905 | 350.792 |
| Restore to first count | 693.228 | 689.167–698.130 | 701.015 |
| Wait for no-change scan | 885.597 | 883.453–895.995 | 878.795 |

Peak RSS was 1,121,340–1,179,484 KiB across the three final processes, compared with 1,084,812–1,149,180 KiB in the initial run. The new digest and cache behavior did not produce a material build or mutation latency regression in these three warm-cache samples. Compaction temporarily retains the old and new index: the roughly 1.1 GiB process peak is materially above the roughly 0.5 GiB fresh-build footprint. No source-content reads occurred in the no-change reconciliation.

For a reproducible lifecycle query run with the committed runner, use `cargo run --release -p objekte-search-core --example bench_lifecycle_musenalm -- <fixture-copy> <output.jsonl>`. Set `LIFECYCLE_CASES=Q172,Q097,Q129` for the targeted comparison. The committed runner has the same logic as the ignored runner used for the recorded results.

## Maintenance and batch stress follow-up

The committed example `crates/core/examples/bench_lifecycle_mutations.rs` gained `stress` mode on subtask commit `876fc4e909`. The final three runs used integration `20c04e23cd` merged into the benchmark branch at `39c762eac4`, rustc 1.99.1 release, and three fresh disposable copies of the same 148,486-file Musenalm fixture. The page cache was warm. Each copy included `data/` and `.objekte/collections/`, without old caches; the example mutates only its copy and refuses to run unless `OBJEKTE_BENCH_MUTATE_COPY=1`. Raw TSV and `/usr/bin/time -v` output are `/home/simon/source/objekte-integration-index-lifecycle/target/lifecycle-final/stress/lifecycle-stress-run-{3,4,5}.{tsv,time}`. The pre-fix diagnostic run is `target/lifecycle-final/stress/lifecycle-stress-run-2.{tsv,time}`.

Repeat from the retained integration worktree:

```bash
cargo build --release -p objekte-search-core --example bench_lifecycle_mutations
for n in 6 7 8; do # choose unused run numbers for fresh copies
  mkdir -p target/lifecycle-stress-run-$n/.objekte
  cp -a --reflink=auto /home/simon/source/objekte-integration-index-lifecycle/target/lifecycle-fixture/musenalm/data target/lifecycle-stress-run-$n/
  cp -a /home/simon/source/objekte-integration-index-lifecycle/target/lifecycle-fixture/musenalm/.objekte/collections target/lifecycle-stress-run-$n/.objekte/
  OBJEKTE_BENCH_MUTATE_COPY=1 /usr/bin/time -v target/release/examples/bench_lifecycle_mutations stress target/lifecycle-stress-run-$n > target/lifecycle-stress-run-$n.tsv 2> target/lifecycle-stress-run-$n.time
done
```

The stress sequence first measured a true base-file `PreferVerified` update, followed by eight-file and 128-file append batches, 32 successive updates to one file, metadata reconciliation, compaction, rebuild, flush, restore, and no-change reconciliation. Source bytes were changed before timing each batch; batch time covers index API calls, while repeated-update wall time also includes the 32 source writes. A separate `agents` locator page at offset 500 and limit 50, with no count or sort, ran on a second thread during each long maintenance action, sleeping 2 ms between queries. Its steady-state sample was 500 ms. This probe checks response availability under maintenance; the established sorted/count pagination suite above measures the richer query routes.

| Phase | Median wall ms | Range ms | Other observed result |
| --- | ---: | ---: | --- |
| Fresh build + count | 1,317.728 | 1,281.419–1,322.395 | 148,486 files and records |
| Base append update | 0.329 | 0.263–0.464 | append replacement |
| Base verified update | **1,278.918** | **1,246.095–1,299.952** | verified physical removal, no fallback |
| Eight-file append batch | 3.861 | 3.677–3.923 | per-update p50 0.290 ms, p95 0.543 ms (median across runs) |
| 128-file append batch | 44.207 | 41.743–45.847 | per-update p50 0.328 ms, p95 0.489 ms |
| 32 repeated updates | 8.415 | 7.667–11.709 | per-API p50 0.235 ms, p95 0.248 ms; wall includes writes |
| No-change reconciliation with queries | 910.805 | 902.467–919.608 | 148,486 examined and unchanged, zero issues |
| Compaction with queries | 1,122.551 | 1,106.229–1,124.823 | 136 retired slots removed; 148,486 slots after |
| Rebuild with queries | 1,230.018 | 1,224.725–1,266.460 | one attempt, current count preserved |
| Flush with queries | 307.026 | 300.229–394.895 | 154,725,738 bytes; serializer median 262.554 ms |
| Restore to searchable count | 434.281 | 395.872–508.908 | 133,948 contents |
| Wait for restored no-change reconciliation | 859.117 | 858.210–862.307 | 148,486 unchanged; zero issues |

Query durations are per-run p50/p95/p99; the table reports their medians over three runs. Each query reacquired the current published view. There were zero errors and the page count stayed stable throughout.

| Probe phase | Samples per run | p50 ms | p95 ms | p99 ms | Largest observed ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Steady | 240–241 | 0.0158 | 0.0849 | 0.1017 | 0.252 |
| Reconcile | 331–336 | 0.6710 | 0.7413 | 0.9446 | 1.797 |
| Compact | 398–403 | 0.7003 | 0.9260 | 1.1485 | 1.936 |
| Rebuild | 400–424 | 0.7612 | 2.2682 | 3.2192 | **73.896** |
| Flush | 145–190 | 0.0172 | 0.0506 | 0.0694 | 0.086 |

The rebuild maximum was 72.677–73.896 ms in two runs and 2.935 ms in the third. The p99 stayed at or below 3.676 ms, so this is an isolated tail event rather than a typical query time; its cause was not instrumented. Reconciliation and compaction lifted the probe median from 0.016 ms to about 0.7 ms while searches remained available. The probe's concurrent CPU and filesystem activity contributes to maintenance wall times. Whole-process peak RSS was 1,435,140–1,478,780 KiB (median 1,448,836 KiB). After verified base maintenance the observed HWM was 1,030,944–1,072,768 KiB; it rose to 1,352,272–1,397,160 KiB by rebuild and reached the process peak by flush. These observations include temporary old-plus-new roots and allocator retention; the harness does not separately count live allocations or lock hold times.

The pre-fix run on integration `e77f8483a4` exposed a material interaction: the base verified update took 1,244.951 ms and the later 128-file batch took **73,039.912 ms**, with per-update p50 949.311 ms/p95 1,058.371 ms. At the 64-segment limit, `PublishedIndex::make_room_for_segment` eagerly compacted a partially retired 148k-record shard before merging small segments. Integration `20c04e23cd` changed admission to merge the cheapest compatible pair first. The same 128-file phase fell to the 41.743–45.847 ms range across all three final runs. The base-file verified operation still costs about 1.28 seconds and collection-sized memory, so it is a physical maintenance choice, not a low-latency optimization. Final runs did not measure source metadata syscall counts, per-operation allocator bytes, or exact publication lock duration; those require profiling hooks beyond this bounded example.
