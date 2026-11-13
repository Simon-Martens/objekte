# Snapshot loading and admission campaign

This archive belongs to task `snapshot-loading-admission-profiling`. The analysis is in [the generated report](../../../docs/ai/generated/snapshot_loading_admission_profiling_results.md). `summary.json` is regenerated from the individual `runs/*/trial-*/sample.json` files by `python3 summarize.py`. Quartiles use inclusive quantiles. The 68 archived trials include 28 primary ASCII restores, 21 primary constructions, 12 additional ASCII profiled restores, and 7 Unicode cross-check restores. The two snapshot-creation samples are under `fixture/` and `unicode-fixture/`.

## Revisions and files

| Variant | Code commit | Change |
|---|---|---|
| B0 | `6c808ebb70203d2442a5e8f2f595199ae5f8d171` | Integrated prerequisite baseline |
| B1 | `7ac10f36cd762708aa943a70fa4700ffb9592d5c` | Inventory construction once |
| B2 | `55a38262b28af74cb5858c4f4478385ff4b15804` | Bigram subset shortcut only, on isolated branch |
| B3 | `93a98cc4dc578dde768c3a83e41b97d36f33e9b2` | Both optimizations and admission instrumentation |

The binary hashes, machine configuration, corpus fingerprints, snapshot hashes and sizes are in `environment.json`. Each trial has `sample.json` (raw measurements), `run-manifest.json` (benchmark metadata), and `trial-meta.json` (variant source commit, binary hash, snapshot hash, exact command). The benchmark's built-in `run-manifest.json.code_revision` names the **runner checkout** B3 for copied B0/B1/B2 binaries. Use `trial-meta.json.source_commit` and `binary_sha256` to identify the actual executable. The Unicode fixture's built-in manifest has the same runner-revision limitation; its recorded command and `environment.json` identify the B0 binary. The final integration HEAD also contains a later tests-only commit; the release B3 binary was built at the commit above.

Large local binaries and snapshots are intentionally excluded from Git. The ASCII snapshot is lifecycle format 28, 132,710,084 bytes, SHA-256 `2a12d69cdc64a3e78e0af719138f5c2a445615f970d6cffb7f17623716b32ee2`. The Unicode snapshot is format 28, 135,240,992 bytes, SHA-256 `f5ced9b1c9af5f85a8fa43cafef7236a6024299c1bfa0c323076e7ee30ed5d7f`. Both were generated once outside restore timing with the B0 binary, `--incremental --snapshot --check-queries`, the relevant alphabet, 32 workers, and the configured Musenalm root. Recreate them before rerunning restore trials; the scripts expect the local `fixture/trial-*/index.snapshot`, `unicode-fixture/trial-*/index.snapshot`, and `local-binaries/{b0,b1,b2,b3}` paths.

## Reproduce

Build each executable at its listed commit with:

```sh
cargo build --release --locked -p objekte-search-core --example bench_index_construction
```

Copy `target/release/examples/bench_index_construction` to the corresponding ignored `local-binaries/<variant>` path. The recorded fixture-generation command is in each fixture `run-manifest.json`. Then, from this directory, run the following sequentially, without concurrent compilation or benchmarks:

```sh
python3 run_campaign.py
python3 run_profiled_individual.py
python3 run_unicode.py
python3 summarize.py
```

The runner uses `taskset -c 0-31`. `run_campaign.py` interleaves and rotates B0–B3 primary restores, B0/B3 profile-off construction, B3 profiled construction, and three B0/B3 profiled restores. `run_profiled_individual.py` adds three profiled B1/B2 restores. `run_unicode.py` adds three B0/B3 Unicode pairs and one B3 profiled Unicode restore. Each invocation is a new process; the filesystem cache was warm and no host-wide cache was cleared. Queries run after the measured build or load interval.

`campaign.log`, `profiled-individual.log` and `unicode.log` contain rounded console progress. The `sample.json` nanoseconds and `summary.json` are authoritative. `/proc/self/io` counters apply to the measured interval; physical `read_bytes=0` describes this warm-cache run and is not a cold-start prediction.
