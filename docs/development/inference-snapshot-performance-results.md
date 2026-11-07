# Inference and snapshot performance results

Measured 7 November 2026 on the `integration/inference-snapshot-performance` branch. The implementation started at `4fd47bcccd1affab1f98fff1db5cff21e6b73ce5`. The [contract](inference-snapshot-contract.md) defines the load and explicit-save APIs. Raw JSON, stderr, `/usr/bin/time -v` output, reproducible commands and paired-sample CSVs are in [the result directory](../../bench/results/musenalm/inference-snapshot-performance-20261107/). The generated 100,000-record input is reproducible with [the generator](../../bench/generate_inference_fixture.py); the 42.1 MB input itself is not checked in. Its SHA-256 is `0a3c614d3226c9bdcc964d4523949d292ef666077b18dee54124ea45ebe59373`.

## Persistence and correctness

`Repository::open()` now defaults to load-if-valid, builds in memory when needed, and never writes. `Disabled` builds without loading; `Required` restores or errors. The caller persists a generation explicitly:

```rust
let repository = Repository::open(&root, definitions)?;
repository.save_snapshot(&snapshot_path)?;
```

The CLI implements `--write-index` by saving after a fresh build. With both index flags, it restores without rewriting a valid snapshot and saves after rebuilding. The CLI TUI uses the same opening path; its ready and saved statuses have separate timings. An unsaved rebuild must be repeated on the next open. The CLI policy and directory rules are in [the user guide](../cli/obj.md#snapshot-policy).

Tests cover absent/valid/stale/corrupt snapshots, required restore, zero source parses on restore, no implicit replacement, a publication failure preserving the old bytes and modification time, and cleanup of the temporary file. `save_snapshot_with_stats` retains `File::sync_all` and atomic rename. Its one-pass bincode encoding is byte-compatible with the earlier codec for the tested values; snapshot round trips and corruption checks pass. The latest AutoIndex fixture produced the same report (apart from the intentional inference policy version 4 to 5) and identical index diagnostics as the pre-optimization fixture: 12 observed paths, six admitted indexes, 100,000 parsed records and 100,000 distinct UUID keys.

## Inference construction

The Musenalm quick-build fixture uses `data/entries`, ASCII grams, release profile, snapshots disabled, auto workers (32 resolved), no queries, and the same schema throughout. Its corpus fingerprint is `0ad10ab59d75a9f667f49a1b4b2d6cc4dc6743af95874479973e0458db5ac644`. Each row is **one** build; small differences are inconclusive. Time is core `build_ns`, excluding process startup and later queries.

| Code stage | SHA | Disabled ms | ReportOnly ms | AutoIndex ms | Observed / admitted |
| --- | --- | ---: | ---: | ---: | ---: |
| Base after load-only opening | `f7a4d75ce3` | 53.075 | 68.645 | 102.612 | 65 / 58 |
| Reused path / field-name interning | `171a3ff4df` | 51.931 | 64.276 | 98.197 | 65 / 58 |
| Incremental distinct-byte accounting | `e4c22a7b71` | 62.393 | 67.358 | 104.288 | 65 / 58 |
| Early manual/opt-out posting policy | `44e690f6d7` | 56.366 | 71.741 | 99.510 | 65 / 58 |
| Fused traversal experiment (reverted) | `da33390371` | 60.108 | 71.897 | 94.794 | 65 / 58 |

The repeated 100,000-record fixture better isolates the early policy change. It has manual and explicitly prohibited 100,000-distinct-value fields alongside eligible short strings, nested paths, arrays, free text, and 65-byte outliers. Seven alternating paired runs, after warm-ups, compared `e4c22a7b71` with `44e690f6d7` using the same generated file and auto workers. Both built the same six indexes and parsed all 100,000 records. Median build time changed from **337.995 to 293.117 ms** for AutoIndex; median paired delta was **−44.036 ms**. Median peak RSS changed from **254.668 to 203.992 MiB**; median paired delta was −50.676 MiB. ReportOnly medians were 199.147 and 196.331 ms (paired −2.603 ms), within variation. Disabled medians were 150.795 and 156.629 ms (paired +13.250 ms), with individual deltas ranging from −11.391 to +21.772 ms; no disabled-path change was made by the policy check. The separate release runs are not allocator counts; the code removes repeated path-name/string allocations and provisional posting payloads, while RSS gives only a process-level memory observation. Actual active-job and admission counters are in the raw JSON.

The fused logical-text/inference traversal at `da33390371` preserved text segments, empty-array facts and index reports in tests, including budget exhaustion. Seven more alternating pairs against `7c85ced384` showed AutoIndex build medians 315.090 to 300.145 ms, but individual paired deltas ranged from −25.102 to +16.283 ms; ReportOnly medians moved 203.314 to 209.511 ms. The coupling added 177 lines and did not show a reliable ReportOnly gain. It was reverted by `345498c474`; those samples remain in `fusion-samples.csv`.

The coordinator drops values once when observation becomes incomplete. Already dispatched worker batches can still build local observations after ordered admission determines that a collection is incomplete. Suppressing their work based on completion order would change report facts or admission under different schedules; this bounded remainder was left in place rather than adding a scheduling-dependent cancellation path. No new global lock or per-value signal was added.

## Snapshot attribution and repair

The primary historical comparison disabled inference and used the same seven explicit Musenalm collection definitions, ASCII grams, release build, 32 workers and immutable 148,486-file corpus. The historical binary was built from `f374101b4839f28199a99677118ac96869203bb0`, with only the measurement probes from `historical-storage-probes.patch`; the new binary was `fb19dbb22c` before the serializer change. Both opened/build the repository **outside** the explicit save timer. Their schema-support file has SHA-256 `caa5e8e5a3b5789616f73dfb3ee3658da4c33f9d619bee5bb3ca0a2623901c8c`. Seven alternating pairs followed a warm-up for each binary. The snapshot grew from 111,214,089 to 111,255,130 bytes, only 41,041 bytes.

| Phase, median ms | Historical | Current before repair |
| --- | ---: | ---: |
| Total explicit save | 269.611 | 250.343 |
| Section serialization | 139.593 | 138.033 |
| Checksums | 41.902 | 41.525 |
| Writes | 14.304 | 14.723 |
| `sync_all` | 38.468 | 37.742 |

The median **paired** total delta was +9.687 ms; the individual paired deltas ranged from −214.951 to +90.091 ms. Both sides had large write/sync outliers (historical write up to 236.657 ms; current sync up to 143.096 ms). Section 2, the largest collection, took about 108 ms to serialize on both sides before the repair. The previously reported 38–40% snapshot-write increase was **not reproduced** under matched explicit-save conditions. This identifies no historical serializer regression; it does not prove a particular external I/O cause.

The pinned bincode 1.3.3 `serialize` implementation sized the value and then encoded it. `de104fdc56` uses `serialize_into` directly into the existing section vector, preserving the exact section bytes, SHA-256 checksums, `sync_all`, and rename. Seven alternating same-code pairs (`fb19dbb22c` versus `de104fdc56`, after warm-ups) all improved: median total explicit save **233.327 to 196.784 ms**, median paired delta **−32.181 ms**; section serialization **136.512 to 94.730 ms**, median paired delta −40.931 ms. Both snapshots were exactly 111,255,130 bytes. Median peak process RSS was 594.0 versus 567.6 MiB, but the ranges overlapped (516.0–598.1 versus 513.7–606.2 MiB), so there is no supported memory-reduction claim. The improvement is an encoding CPU reduction, not a removal of durable synchronization.

## Environment and limits

`rustc 1.99.1`, release profile, default flags, Intel i9-13900K, 32 logical CPUs, CPU governor `performance`, affinity CPUs 0–31. Full environment is in `environment.json`; the exact invocations are in `run-paired-saves.sh`, `run-codec-pairs.sh`, and `run-inference-pairs.sh`. Filesystem caches were not dropped; paired order alternated. Musenalm stage timings are single observations. No global allocator probe was added, so allocation count and allocated-byte totals are unavailable. The policy comparison's useful index counts, coverage, complete reports and source parse counts were checked independently of its time.
