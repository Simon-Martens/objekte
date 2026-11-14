# Hybrid store generation, stamp, and posting proof checkpoint

Implementation branch: `integration/hybrid-index-storage`. Base revision: `ca183a1ed8` (the actual head when this series started); Change C began at `c977710f2d`. The original checkout stayed on `master`. This is a continuation of the broader hybrid index storage task, not its completion record.

## Disposition

| Change | State | Evidence |
| --- | --- | --- |
| A: immutable generation directories | Implemented and tested | Manifest v2 points to one synced directory; readers open without a lock and retry vanished generations; writers hard-link reused segments; count and age grace GC is bounded to 64 entries per pass. Concurrency, inode, migration, orphan, symlink, unknown-entry, and 32 update tests pass. |
| B: persistent validation stamps | Implemented and tested | Producer-proven segments carry a validator stamp. Untrusted v1 migrations and unstamped v2 segments undergo full validation before a compare-and-swap metadata stamp. A matching stamp skips eager leaf order traversal, while framing, section checksums, descriptor bounds, and posting checks remain. Version mismatch and publication races are tested. |
| C: posting proof reuse | Implemented and tested | The per-dictionary mutex/hash/Arc validation map is removed. Cache entries are keyed by descriptor and eligible domain. A dense byte-per-entry ordinal proof table handles repeated small postings; the first access remains checked. Large cache eviction revalidates. |

The descriptor ordinal is implicit in the checked ordered-leaf entry position and is not added to the 24-byte on-disk descriptor. Profiling across Q167/Q175/Q177 observed 223,510 sub-64-byte accesses, 257,238 larger accesses, and 254,236 large cache hits. That repeated small-access volume justified the dense table. It costs one atomic byte per external dictionary entry; constructed dictionaries allocate no table. The old per-dictionary validation map is gone. The decoded posting cache remains bounded by `min(segment encoded bytes / 20, 8 MiB)` per segment.

## Paired measurements

Seven fresh-process construction/flush pairs and seven pairs per stored-open mode alternated order. Query measurements used three alternating pairs, five repetitions over 220 cases × ASCII/Unicode, and three 30-repetition tail pairs. `summary.json` is recalculated from the archived CSVs by `scripts/summarize_pairs.py`; `raw/` contains commands, run manifests, digests, compressed individual measurements, and trial records. A ratio below 1 is faster.

| Measure | Base | Candidate | Candidate/base |
| --- | ---: | ---: | ---: |
| Memory construction, ASCII median | 1,139.79 ms | 1,131.55 ms | 0.993 |
| First flush, ASCII median | 270.54 ms | 294.30 ms | 1.088 |
| Warm on-demand open, ASCII median | 383.06 ms | 383.74 ms | 1.002 |
| Warm fully resident open, ASCII median | 438.68 ms | 439.38 ms | 1.002 |
| On-demand resident RSS after open | 265.28 MiB | 265.15 MiB | 1.000 |
| Fully resident RSS after open | 317.63 MiB | 317.77 MiB | 1.000 |
| First flush logical store bytes | 128,312,320 | 128,312,322 | 1.000 |
| First flush physical allocated bytes | 128,335,872 | 128,335,872 | 1.000 |

All stored opens recorded zero source reads and zero rebuild parses. The 2-byte logical increase is the v2 manifest. The new first flush is 8.8% slower in this paired set, so the flush target is unmet; the additional durable generation creation, directory syncs, and manifest publication are plausible contributors, but this campaign does not isolate their costs. Warm opens are effectively at parity with the flat-store baseline, not measurably faster. The initial two-pass stamped bigram opener caused a measurable open regression; building its absence filter during the framing pass recovered parity.

Seven deliberately unstamped v2 opens validate and stamp eight segments each: on-demand median 401.18 ms and fully resident median 462.21 ms. Compared with the stamped medians above, subsequent opens are 4.3% and 4.9% faster respectively. Each first-open status reported eight fully validated segments and eight stamps written; subsequent status reported eight stamp-trusted segments and zero fully validated. This comparison uses the same v2 layout and resets only validation-version fields before each first-open run.

The complete query campaign returned matching digests for all 440 case/profile combinations in every pair, with zero failures and zero verified-result-cache hits. The geometric mean of per-case median candidate/base ratios is **1.0149** (1.5% slower); the three full-campaign pair geomeans are 0.9925, 1.0483, 1.0274. The 30-repetition selected tail has a geometric mean of per-case median ratios of **0.9918** (0.8% faster); its three pair geomeans are 1.0316, 1.0002, 0.9289. These small differences vary across pairs. Q167/Q175/Q177 and the scanner cases remain for the separately authorized query optimization plan. The no-regression target for Change C is not demonstrated by the complete campaign; the user directed that small differences be documented and the work continued.

The on-disk sharing test confirms an unchanged segment has the same inode in two generations, a link count of two, zero new segment bytes, and one reported link. The 32-update test ages directories to exercise GC without a two-minute wait: 25 removed, eight retained, latest generation readable. This is a retention bound test, not a 32-update corpus throughput benchmark. Real fast updates retain young generations for at least 120 seconds; disk use can temporarily exceed eight generations by the bytes of new segments. No full segment copies are made for reuse.

## Reproduction and provenance

- Host: Intel Core i9-13900K, 32 logical CPUs, Fedora kernel `7.2.15-200.fc44.x86_64`, `performance` governor, Rust `1.99.1`; Cargo.lock SHA-256 `e8915ae2f6dc40433bf346ac0060f4a58a130ae17688801f24da0b744ed8d200`.
- Fixture: `examples/musenalm`; corpus fingerprint `ad517d15e3216bfbdb569906d14a1717271daaf27a8d58e47e55e1e63e5db868`; schema fingerprint `2edaa7c59a2f5d7aad8b8b6cb6065420a23c0a12db3a6b3133eb5bfdbbacf819`; workload fingerprint `b0a7ddfadd81939c89333097a4fdb7ff240953afff1a0559c183c29f40ea99f0`.
- Query command shape: `bench_musenalm --root examples/musenalm --extended --incremental --profiles ascii-off,unicode-off --verified-cache-bytes 0 --prepared-cache-bytes 16777216 --query-workers 32 --repetitions 5 --storage custom <store> --result-dir <dir>`; tail adds `--cases Q001,Q042,Q167,Q175,Q177,Q202 --repetitions 30`. Exact commands and run order are in `raw/paired-*/trials.json` and the run manifests.
- Construction: `bench_index_construction --root examples/musenalm --result-dir <dir> --alphabet ascii --workers 32 --incremental`. Flush: `bench_managed_flush --root examples/musenalm --output <store> --alphabet ascii`.
- Base executable SHA-256: query `dba1d4e5185d9d87f03fd4f8e372a8a62b02d89d81fe64d918c7d0ce32685523`; construction `776716ca0feca95ca4c0ad846af478c901b174712707d628e5b7b2a3718bd5d5`; flush `67f12476ad8d19aff2388997f7f291bae07f058e9d77767b92bfe56264d3ba7e`.
- Candidate executable SHA-256: query campaign `1024bbccb3f609231dad5f84b638e91591bda510077d319616c4f265a8f79a7b`; construction `5943e846d7512c229a9ac890d4035eb8d679ce6b1e0ad520bb05e8a10df164e0`; flush `ab79244c7941ae926d5a4a1e4d5f88c0ebd0d771384d4a2256bc561ed70890bf`. The later single-pass stamped opener was used for the open pairs; query hot-path code was unchanged after the paired query campaign. Run manifests identify their exact binaries; their `code_revision` is the checkout revision, not necessarily the binary build revision.

## Validation and remaining limits

`cargo test --workspace --locked --quiet` passed after the generation/orphan repair; the focused 32-update test and `cargo test --locked -p objekte-desktop --example environment_validation --quiet` also passed. `cargo clippy --workspace --all-targets --locked --quiet` completed with warnings, including inherited warnings. `git diff --check` passed. The repository-wide `python3 scripts/format-rust-spacing.py --check` currently reports formatting drift in unrelated files; touched Rust files were formatted with the repository workflow and unrelated formatting edits were restored. CLI/desktop storage tests are included in the workspace run.

The full fault-injection matrix in the specification is broader than these tests: a foreign-owned file and every dictionary-specific corrupted posting route were not individually injected. The stamped trust path still checks section and posting checksums, and core corruption/domain tests pass. Warm-cache timing does not establish cold-index or constrained-memory behavior. Full query medians and first-flush time show small regressions against the stated targets; they remain visible in the archived results. The broader hybrid index storage specification retains separate unfinished work, and the authoritative task documents stay in `docs/ai/tasks/`.
