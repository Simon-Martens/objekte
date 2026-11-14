# Direct snapshot access: final measurement archive

The paired baseline is `dd3f8feb19` on `integration/hybrid-index-storage`; the candidate implementation is `8747efc47e` on `integration/direct-snapshot-access`. Commit `175c94bc15` adds feature-gated phase instrumentation and extra fields to an example; the ordinary release implementation measured here has the same behavior. Executable SHA-256 values, absolute source/store paths, commands, trial records, and per-case results are in the JSON files. These measurements were made on Linux 7.2.15-200.fc44.x86_64, Intel Core i9-13900K, Rust 1.99.1, with the Musenalm fixture in `examples/musenalm` (148,486 source records), 32 query workers, zero verified-cache bytes, and 16 MiB prepared-cache budget.

| File | Contents |
| --- | --- |
| `queries-summary.json`, `queries-per-case.csv`, six `*.jsonl.gz` files | Full ASCII/Unicode, memory/resident/on-demand campaign; 30 repetitions per case, 220 cases per profile, result digests checked |
| `warm-opens.json` | Seven paired source-free opens per mode/profile; includes first Q167, RSS, and managed bytes |
| `builds.json` | Seven paired fresh builds |
| `flush.json` | Seven paired initial and unchanged flushes |
| `updates.json` | Three paired first updates after restore on a 16,384-file JSONL fixture |
| `cold-opens.json` | Three paired opens after `POSIX_FADV_DONTNEED`; cached bytes checked with `fincore` |
| `allocations.json` | Seven paired opens per mode/profile with the example's counting allocator |
| `open-phases.json`, `*.stderr` | Three candidate phase traces per mode/profile from a `detailed-profiling` release build |

The runners are in `../scripts/`. The report at `docs/ai/generated/direct_snapshot_access_report.md` interprets the results. The cold experiment evicted file pages but did not reboot or isolate the machine; it is not a cold-boot claim. Instrumented phase times are explanatory and are not the acceptance timings.
