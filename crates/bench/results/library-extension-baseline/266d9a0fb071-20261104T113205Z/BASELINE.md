# Objekte library extension reference baseline

## Frozen source and environment

The unchanged production engine is commit `266d9a0fb071838be8f757671599872a7036a054`. The worktree was clean before benchmark-only additions. This is newer than the design document's inspected `f58d556bde24f493cae8fd8e02faefafd4351da2`; no reset was performed. `manifest.json` records the exact starting revision, lockfile SHA-256, environment, features, workload checksums, binary hashes, and raw artifact hashes. `harness.patch` and the manifest's script hashes pin the benchmark-only additions. Production core, CLI, and documentation were unchanged when this report was written.

Host: Intel Core i9-13900K (32 logical CPUs), 30 GiB RAM, Fedora Linux on btrfs/NVMe, Rust/Cargo 1.99.1, `x86_64-unknown-linux-gnu`, Cargo release profile with `target-cpu=native`, fixed `RAYON_NUM_THREADS=16`. Normal builds use the Rust system allocator; `core-allocation` and `index-build-profile` use counting wrappers over it. Query binaries and the normal CLI control have no allocation instrumentation. The instrumented build profile uses `index-timings`; the query-cache allocation run uses `allocation-profile`. All measurements use warm filesystem page caches. No global OS cache was evicted.

The immutable source inventory and per-file content/stat fingerprints are in `corpora.json`. `corpora-after.json` matches them exactly after measurement. The same source paths and filesystem metadata will be used for candidate runs. The available corpora are Musenalm (148,486 JSON records), Lenz (3,263), Leipzig (400,000), Amazon (1,000,000), and NYC (1,000,000). `manifest.json` records per-corpus bytes, source manifests, schema configuration, and workload checksums. The allocation fixture is a separate disposable 20,000-record corpus under `/tmp/objekte-alloc-fixture-library-baseline`; mutation tests use disposable copies.

Reference executables are preserved under `reference-bin/`, `reference-bin-allocation/`, and `reference-bin-index/`; `reference-binaries.sha256` pins each. Candidate builds must use a separate target or copied executable set. The 20 matching base-plus-secondary snapshots are preserved in `snapshots/`. Runner command lines, stderr, exits, and individual samples are retained alongside each raw output.

The executable and snapshot directories are local generated artifacts excluded from Git. The committed manifest and checksum lists identify them; a fresh checkout must rebuild the executables and snapshots before replaying measurements.

## Coverage and evidence

| Measurement | Raw evidence | Completed coverage |
| --- | --- | --- |
| JSON query mix | `query-matrix/` | 152 current cases, 7 rounds for each schema/no-schema, ASCII/UTF-8-byte, and 0/64 MiB cache combination; 8 experiments. |
| FTS query mix | `query-matrix-tail/` | 30 cases, 7 rounds, 0/64 MiB cache. |
| Sort mix | `query-matrix-tail/` | 50 cases each for original and relevant scope, 7 rounds, full ordered page IDs. |
| Source grep comparison | `query-matrix-tail/` | 16 cases, 7 rounds, existing complete-result checks and paired source grep. |
| Query cache | `query-cache.ndjson`, `query-cache-evidence/` | 3,416 rows / 56 on-off pairs across seven repetitions: edit sequence, 16/32/64 MiB budgets, offsets 25/250/2500, order changes, all 152 mix cases, 30 FTS cases, and selective Leipzig/Amazon/NYC repetitions. |
| Query cache allocation | `query-cache-allocation.ndjson` | Seven instrumented pairs. |
| Bigram encoding | `bigram-encoding/` | Both encodings on Musenalm contents and deterministic multilingual `synthetic:24000`; five fresh processes per combination, 20 outputs. |
| Core allocation | `core-allocation.ndjson` | 24 cases × 5 independent samples, including 20,000-record fixture, Musenalm, source lifecycle and repeated streaming with cache opt-in off/on. |
| Index build | `index-build.ndjson` | Musenalm, Lenz, Leipzig, Amazon, NYC, each base and FTS, five samples: 50 rows. |
| Snapshot restore | `snapshot-restore.ndjson` | Matching ten base/FTS profiles, five samples: 50 rows; complete base-plus-secondary restoration. |
| Normal release control | `normal-index-control/results.json` | Five fresh-process Musenalm-contents reindex and snapshot restore/query runs through the CLI. |

The query matrix contains 1,392 case outputs across 13 experiments; `query-signatures.json` holds semantic signatures. JSON case signatures match across cache and encoding configurations within each schema mode. FTS signatures match across cache budgets. All 56 cache pairs agree and remain stable across their seven repetitions. Bigram candidate signatures are stable across repeated processes. Lifecycle results agree with fresh direct scans after edits, inserts, deletes, renames, and refresh. The 2,000-record schema-bound streaming fixture demonstrates repeated cache reuse: with opt-in off, each of three repeats reads 670 sources; with opt-in on, the first reads 670 and the next two read only three returned-page sources, with the same 667 matches and ordered IDs. The normal release control returns 2,908 matches and the same first three ordered IDs in all five runs, with base and secondary snapshot loads observed.

For a reference-versus-reference control, the unchanged JSON schema/ASCII/64 MiB binary was rerun on all 152 cases. Semantic signatures were identical; the median absolute relative difference of case medians was 0.88%. `reference-repeat-summary.json` preserves per-case differences. The initial and final instrumented index-build passes (both unchanged production code) drifted materially, roughly 12–22% in many profile medians; see `index-build-initial.ndjson` and `index-reference-repeat-summary.json`. The profiler output was extended between passes, and warm host state may also contribute. This is a warning against treating a single build pass as a candidate improvement or regression. Candidate comparisons must alternate reference and candidate in the same session, retain per-case samples, and repeat unresolved differences. Seven observations do not establish stable p95/p99.

## Correctness checks and pre-existing failures

The initial and post-harness `cargo test --locked --workspace --all-targets` and `cargo clippy --locked --workspace --all-targets -- -D warnings` passed. Feature tests for `objekte-core` with `index-timings` and `objekte-bench` with `allocation-profile` passed. Changed benchmark Rust files passed rustfmt. The opt-in release tests `musenalm_json_filter`, `json_query_oracle`, and `musenalm_bigram` passed; the oracle includes 43 independent cases. Logs are retained in this directory.

Three observed failures are part of this baseline:

1. `python3 scripts/format-rust-spacing.py --check` fails in both the initial and post-harness runs, solely at existing differences in `crates/core/src/bigram/filter.rs:1447` and `crates/core/tests/index_build_profile.rs:106`.
2. The opt-in `musenalm` release test fails in unchanged core with `Request("columns must be nonempty and unique")` at `crates/core/tests/musenalm.rs:38`.
3. The original FTS benchmark requests `:fts ~ :ignorecase(...)` in three cases, which the current parser rejects. The original executable and failure log are retained. A benchmark-only adjustment to plain literal FTS terms yielded the accepted 30-case baseline; no production semantics were changed.

The current `musenalm-mix-100.json` contains 152 cases, despite its name. The cache runner still calls that experiment `json100`. Case counts were derived from loaded workloads and outputs. The retained `compare_index_revisions.py` was not used: it targets a removed driver. `core-allocation`'s older `json-updates` and `text-updates` cases measure streamed page updates; the added lifecycle cases cover actual source mutation.

## Reproduction and comparison protocol

Run from the repository root with `RAYON_NUM_THREADS=16`. The exact executed command lines and exits are saved in each runner's evidence directory, and all input/output hashes are in `manifest.json`. The benchmark-only runner options `--bin-dir` and `--evidence-dir` select the preserved reference binaries and capture failures. Use those identical runners for the candidate, on these same corpora and filesystem metadata. Keep normal, allocation-instrumented, and index-timing builds separate. Alternate reference/candidate order and serialize timed work. Compare full signatures, then per-case median, spread, absolute and relative timing changes, source work, allocations, memory, and index/snapshot sizes. A reproducible slowdown must be fixed or reverted; noise remains unresolved until sampled enough.

The saved baseline establishes Gate 0 on this checkout. Its limits are warm filesystem caches, no cold physical-storage measurement, no stable tail-latency estimate from seven samples, and a normal CLI build/restore control limited to Musenalm contents. The known failures above remain open baseline facts rather than candidate regressions.

### Supplemental deterministic streaming-fixture replay

The initial `core-allocation` streaming fixture used implicit filesystem modification times. This made its default newest-first page IDs differ between fresh processes, despite matching counts, cache hits, reads and parses. The fixture was corrected after Gate 0 to assign fixed, distinct modification times to its 2,000 files. The unchanged reference core at commit `266d9a0fb071838be8f757671599872a7036a054` was reconstructed with `git archive`; only the corrected benchmark source was overlaid, and its release `core-allocation` binary was preserved as `reference-bin/core-allocation-fixture-v2`. Its SHA-256 and twenty raw reference/candidate process outputs are recorded in `manifest.json` and `stream-fixture-v2/`. For each cache setting, five independent reference and candidate processes alternated order. Complete ordered IDs and all other signatures now agree across revisions and runs. The original raw rows remain preserved for audit but must not be used for cross-process ordered-ID parity.
