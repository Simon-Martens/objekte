# Hybrid index storage repair: measured checkpoint

Measured implementation: `e15b3f5c4b0efe9b89e8e2789db4fe22a56f72f3` on `integration/hybrid-index-storage`. Original baseline binary source: `da8fde68472b8646a63a05b3b3654c58de877301`. A subsequent error-classification fix is `a9ab5ec284`; it changes managed write failure handling, not construction or query execution. The original checkout remains on `master` and was not modified.

On 13 November 2026 the user directed that small timing differences should be improved and documented, rather than treated as absolute completion limits. The ratios below are therefore reported without claiming that the original numerical gates passed. Correctness and remaining functional work are assessed separately.

## Paired fresh-process results

Seven ASCII pairs used 32 workers on the configured Musenalm corpus, rotating execution order. Values are medians of the seven measured trials; the build includes sealing and complete readiness. All candidates and baseline trials used the incremental backend. `paired/*/trials.json` retains commands, executable hashes, raw sample paths, RSS and first-batch readings.

| Measure | Baseline | Hybrid | Hybrid / baseline |
| --- | ---: | ---: | ---: |
| Complete memory construction | 999.99 ms | 1,130.43 ms | 1.130 |
| First searchable batch | 338.26 ms | 353.77 ms | 1.046 |
| Retained RSS after build | 532.69 MiB | 562.74 MiB | 1.056 |
| Peak RSS during build | 542.22 MiB | 580.82 MiB | 1.071 |
| Stored opening, on-demand | 637.35 ms | 386.26 ms | 0.606 |
| Stored readiness, fully resident | 637.35 ms | 443.28 ms | 0.696 |
| First complete save / flush | 352.01 ms | 236.97 ms | 0.673 |
| Stored bytes | 132.71 MB | 128.31 MB | 0.967 |

Opening is compared with the old snapshot restore using warm operating-system caches. Each hybrid open recorded zero source reads and zero build parses. The old save and new managed flush have different file layouts and durability steps, so the flush ratio describes observed calls, not an assertion of identical durability cost. Opening through a matched first query and controlled cold-index access have not been measured.

## Full query campaign

The existing 220 cases ran in ASCII and Unicode, 30 measured repetitions per case/profile, with the incremental backend, 32 query workers, a 16 MiB prepared-plan cache, and **zero verified-result-cache bytes**. All 440 case/profile pairs returned the same result digests in memory, fully resident and warm on-demand modes; no sample failed or hit the verified-result cache. Stored modes reopened a durable generation without rebuilding from source. The clean memory campaign was rerun from `e15b3f5c4b` to avoid relying on an earlier precommit sample.

| Hybrid mode | Geometric mean of per-case median ratio | Geometric mean of per-case p95 ratio |
| --- | ---: | ---: |
| Memory only | 1.075 | 1.238 |
| Fully resident | 1.091 | 1.116 |
| Warm on-demand | 1.092 | 1.105 |

The p95 geometric means summarize the distribution; individual cases are retained in `summary.json`. For example, Q175 ASCII p95 was 3.71 ms baseline, 5.37 ms memory, 4.51 ms resident and 4.85 ms on-demand. Several short cases have large ratios with tiny absolute differences. Source-heavy Q006/Q007 are about 22–24 ms slower in stored modes. These regressions remain visible and were not compensated with the opening improvement.

**Statistics:** all ratios were recalculated from `measurements.jsonl.gz` by `../scripts/summarize_final.py`. The per-case median is the middle pair mean rounded half up; p95 is nearest rank `ceil(0.95n)-1`; the aggregate is `exp(mean(log(candidate / baseline)))` over all 440 pairs. The baseline binary's CSV used the upper middle sample, while the hybrid CSV used the middle pair mean, so comparing those CSV medians directly would mix definitions. Raw samples and `summary.json` are authoritative here. No repetitions were discarded as warm-up.

## Provenance and reproduction

- Host: Intel Core i9-13900K, 32 logical CPUs, Fedora kernel `7.2.15-200.fc44.x86_64`, `performance` governor, Rust `1.99.1 (58cf8f9c3 2026-10-13)`; `Cargo.lock` SHA-256 `e8915ae2f6dc40433bf346ac0060f4a58a130ae17688801f24da0b744ed8d200`.
- Fixture: `examples/musenalm`, 148,486 source files, 193,297,230 source bytes; campaign corpus fingerprint `ad517d15e3216bfbdb569906d14a1717271daaf27a8d58e47e55e1e63e5db868`, schema fingerprint `a7cff399f67082c6153b1245b24a8a392a50a5dd69563762047b5c15ec944433`, workload fingerprint `b0a7ddfadd81939c89333097a4fdb7ff240953afff1a0559c183c29f40ea99f0`.
- Release binaries built with `cargo build --release --locked -p objekte-search-core --example bench_index_construction --example bench_musenalm` and, separately, `--example bench_managed_flush`; no `detailed-profiling` feature in the timed candidate binaries.
- SHA-256: old build binary `a39a37dc050aa2a499542661ef5d3a2233314da00f01336696eb330018edb642`; old query binary `01deffe55c7c7a343d208aeb4453f8b1202b0688945175fd54e1bd851320a048`; hybrid build `accf30f116715359d5fe2d2446704aba32189e89eda158ca0ff0a54c234bce78`; hybrid query `18503dc21e560a6414ea8e52690210920670ea2b76f653639e73a1c17bd37a87`; hybrid flush `2067ba11ce3a7e652b02914cf99514553865aea2657b37a5dfce97188f37768f`.
- The archived query `run-manifest.json` files contain full commands and resolved settings. Their `code_revision` describes the execution checkout, **not** the old binary's build revision. The build revision and hashes above identify the binaries independently. `paired/*/trials.json` contains the paired commands and all seven samples per role. The managed store fixture is reproducible via the first-flush example; its 128 MB immutable files were not committed.

## Verification and remaining work

At `e15b3f5c4b`, `cargo test --locked --workspace --all-targets --quiet` passed, including 640 core unit tests (16 ignored), 166 desktop library tests (1 ignored), 89 CLI library tests and 35 CLI process tests (1 ignored). `cargo clippy --locked --workspace --all-targets --quiet` passed with warning-level diagnostics. The same workspace and Clippy commands passed again after `a9ab5ec284`; focused tests verify typed write I/O versus generation conflict, retained old durable generation, visible failed synchronization and successful retry.

Repair disposition against the supporting specification:

| Repair | State at `a9ab5ec284` |
| --- | --- |
| 1. Owned finalization | Consuming incremental/memory seal and deferred duplicate metadata implemented; synchronous fresh disk path still takes an older sealing route. |
| 2. Validation boundaries | Owned posting and bigram proofs reused; external reads remain checked. Full fault matrix is still incomplete. |
| 3. Direct persistence | Sealed sections are written without posting decode/re-encode; first flush improved in paired trials. |
| 4. Query access | Exact dictionary lookup, compact gram absence filter and bounded decoded cache implemented. The full measured query ratios above remain higher than baseline. |
| 5. Memory | Retained and peak build RSS are 5.6% and 7.1% above baseline in the paired campaign. |
| 6. Maintenance | Independent count/byte/retired triggers, deletion maintenance, shared routing and posting/bigram/relation scatter implemented. Inference projection and explicit byte/allocation/elapsed job budgets remain. |
| 7. Store errors | Read and write/flush categories now distinguish I/O, corruption, incompatibility, resource and publication conflict; failed flush status/retry tested. Some lower-level helpers still return strings, and the complete fault matrix remains. |
| 8. Evidence | Full 30-repetition query campaigns and seven paired build/open/flush campaigns archived. Matched first-query, cold-index, constrained-memory, independent inference and sustained-update measurements remain. |

This is a measured checkpoint, not a completion record. The authoritative task and supporting repair specification remain in `docs/ai/tasks/`; neither was moved to `done/`. The integration branch remains separate from `master`.
