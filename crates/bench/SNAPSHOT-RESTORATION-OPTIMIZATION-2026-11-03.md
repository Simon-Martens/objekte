# Snapshot restoration optimization — 2026-11-03

## Method

Each profile is the median of five separate release-mode restore processes. The runner prepares snapshots once, then restores them with `FilePicker`'s unchecked early-cache path. Data was local and largely in the operating system's page cache. These timings **exclude source-file freshness inventory**. Peak RSS is the process high-water mark; peak live heap is the counting allocator's largest increase during restoration. Sizes below are decimal MB, except RSS in MiB.

```sh
python3 crates/bench/run_snapshot_restore_profiles.py --runs 5 \
  --output target/snapshot-restore.ndjson
```

The runner records stage wall time, allocations, allocated bytes, peak live heap, peak RSS, snapshot bytes read, file count, and posting-column count. Its build uses the `index-timings` feature. Stages measured inside decoding may overlap parent stages, so their durations must not be added together. The baseline and each retained step have separate [raw profiles](results/snapshot-restore-baseline-2026-11-03.ndjson), [compact-manifest profiles](results/snapshot-restore-compact-2026-11-03.ndjson), [direct-slot profiles](results/snapshot-restore-direct-slots-2026-11-03.ndjson), and [final profiles](results/snapshot-restore-sparse-fast-2026-11-03.ndjson).

## End-to-end results

| Profile | Files | Baseline ms | Final ms | Change | Baseline RSS MiB | Final RSS MiB | Base snapshot MB, before → after | Secondary snapshot MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Musenalm base | 133,948 | 376.1 | 310.1 | −17.5% | 161.3 | 161.2 | 25.3 → 22.5 | 32.0 |
| Musenalm FTS4 | 133,948 | 429.6 | 359.0 | −16.4% | 192.7 | 192.7 | 22.3 → 19.5 | 68.9 |
| Leipzig base | 400,000 | 300.8 | 288.0 | −4.3% | 180.0 | 128.1 | 62.1 → 53.7 | 1.9 |
| Leipzig FTS1 | 400,000 | 339.2 | 314.3 | −7.3% | 176.7 | 158.5 | 25.9 → 17.6 | 75.1 |
| Amazon base | 1,000,000 | 816.9 | 761.8 | −6.7% | 489.2 | 363.3 | 210.5 → 189.6 | 4.9 |
| Amazon FTS2 | 1,000,000 | 1,070.0 | 993.8 | −7.1% | 580.1 | 534.3 | 121.7 → 100.8 | 287.9 |
| NYC base | 1,000,000 | 716.2 | 674.8 | −5.8% | 454.8 | 328.8 | 175.5 → 153.9 | 4.9 |
| NYC FTS3 | 1,000,000 | 1,014.7 | 934.0 | −8.0% | 593.5 | 547.7 | 150.9 → 129.3 | 273.9 |

The base and FTS variants use different configurations and thus have different base-snapshot sizes. Compare sizes within one row. Compressed query-index size and secondary-snapshot size did not change within a profile.

| FTS profile | Allocations, before → after | Allocated MB, before → after | Peak live heap MB, before → after |
|---|---:|---:|---:|
| Musenalm | 1.64M → 1.51M | 298.2 → 260.5 | 160.5 → 173.8 |
| Leipzig | 2.00M → 1.60M | 308.0 → 244.5 | 123.3 → 142.7 |
| Amazon | 5.00M → 4.00M | 681.2 → 568.0 | 319.2 → 342.7 |
| NYC | 5.00M → 4.00M | 602.9 → 488.9 | 285.2 → 293.4 |

The compact representation reduces total allocation volume and process RSS, but the measured peak *live heap delta* increases on the FTS profiles. The final ID arena and decoded secondary filters overlap in lifetime more than the old temporary manifest did. This is a real tradeoff; the substantial base-only RSS reductions and smaller FTS RSS on Leipzig, Amazon, and NYC justify retaining it.

## Step decisions

| Step | Decision and measured contribution |
|---|---|
| 0. Stage instrumentation | **Retained.** Feature-gated timers and isolated allocation/RSS profiling exposed the manifest, secondary filter, and sparse validation costs. No production-path timing overhead without `index-timings`. Commit `8c17c805ec`. |
| 1. Direct dense-array reads | **Rejected.** Five runs showed no consistent end-to-end benefit: Amazon FTS 1,070.0 → 1,074.1 ms and NYC FTS 1,014.7 → 1,008.9 ms. Filter decode was essentially unchanged. The prototype was reverted; the existing endian-safe decoder remains. [Raw experiment](results/snapshot-restore-direct-dense-2026-11-03.ndjson). |
| 3A/3B. Bitmap column and permutation validation | **Rejected.** A `Vec<bool>` column table and reused compact permutation scratch reduced a small number of allocations but did not materially change secondary validation (about 9 ms for Amazon/NYC). Leipzig FTS went 339.2 → 349.1 ms. The prototype was reverted. [Raw experiment](results/snapshot-restore-validation-2026-11-03.ndjson). |
| 2. Compact manifest | **Retained.** New `OBJSNP05` stores identifiers in one arena, timestamps as `i128`, and Unix identity as fixed-width numeric fields. It removes per-file manifest strings and shrinks the base snapshot. FTS medians after this stage: Musenalm 369.5, Leipzig 332.4, Amazon 1,039.2, NYC 966.0 ms. Commit `72855307bd`. |
| 2. Direct picker slots | **Retained as part of the compact-manifest commit.** The cache path decodes directly into final `FilePicker.files` and moves the identifier arena into the picker. Slot construction falls from 16.0/43.6/108.7/107.9 ms to 0.3/0.7/2.1/2.1 ms in Musenalm/Leipzig/Amazon/NYC FTS. Some time shifts into manifest decode because stamp fields are now formed there. Complete FTS medians become 364.6/320.5/1,019.7/950.9 ms. The public `BigramSnapshot` materialization API remains available. |
| 2. Parallel slot formatting | **Rejected.** The prototype increased Amazon FTS from 1,039.2 to 1,138.5 ms and NYC FTS from 966.0 to 1,058.9 ms, apparently through allocation contention. Sequential direct construction was faster and simpler. [Raw experiment](results/snapshot-restore-compact-parallel-2026-11-03.ndjson). |
| 3C. Sparse posting validation | **Retained.** A one-byte varint fast path keeps the exact overflow, ordering, and file-count checks. Sparse-validation medians fall 28.8 → 25.0 ms (Musenalm), 56.4 → 47.7 ms (Leipzig), 183.5 → 159.1 ms (Amazon), and 89.7 → 78.9 ms (NYC). Complete FTS medians become 359.0/314.3/993.8/934.0 ms. Commit `ba85491344`. |
| 4. Single-pass checksummed sections | **Rejected at investigation stage.** The measured first checksum pass is 12.4/11.1/62.9/81.5 ms for the four FTS profiles; the decoding pass also computes SHA-256 (10.3/9.2/54.1/68.1 ms). Removing a full pass without losing pre-allocation validation requires independently authenticated, bounded sections and another snapshot version. The maximum possible gain from eliminating the *entire* first checksum duration is only about 3–9% of total restore time, while section hashing remains necessary. No unmeasured format rewrite was retained. |
| 5. Parallel section decoding | **Rejected at investigation stage.** The current format has no independently authenticated section directory, so safe parallel decoding depends on the rejected section-format redesign. Concurrent secondary-filter allocations also risk higher peak memory. No decoder prototype was retained. |

The public snapshot writer additionally checks aggregate ID-arena length and allocation before materialization (`9c8db090d6`). This is a correctness bound, not a measured restore-speed change.

## Stage profile and remaining cost

Final FTS stage medians, in milliseconds:

| Stage | Musenalm | Leipzig | Amazon | NYC |
|---|---:|---:|---:|---:|
| Initial base checksum | 12.4 | 11.1 | 63.0 | 81.5 |
| Manifest decode, including final stamp creation | 60.7 | 161.4 | 401.0 | 408.1 |
| Main filter decode | 17.6 | 0.3 | 104.8 | 89.1 |
| Direct slot publication | 0.3 | 0.7 | 2.0 | 2.0 |
| Secondary structure decode | 203.7 | 3.9 | 9.8 | 10.2 |
| Secondary filter decode | 54.4 | 123.0 | 378.1 | 310.4 |
| Secondary validation | 4.4 | 3.3 | 8.9 | 8.8 |
| Secondary checksum processing | 36.5 | 41.9 | 153.9 | 145.4 |

`sparse_validation_nanos` is a substage of filter decoding, not additional time. Musenalm's secondary structure contains much more view/typed-index data than the other profiles. The manifest remains a large cost, especially for the million-file sets; eliminating its remaining work would need a separate, measured design. The second checksum pass is retained to detect a snapshot that changes while it is decoded. `snapshot_bytes_read` is recorded in every raw profile.

## Correctness and query regression checks

- Full `cargo test --workspace -j 4 --quiet` passed after the final change. `cargo test -p objekte-core --features index-timings --quiet` also passed. Workspace Clippy with all targets, `index-timings`, and `-D warnings` passed; `git diff --check` passed.
- Existing tests cover legacy V1–V4 formats, both encodings in V5, changed-file overlays, public unordered-snapshot behavior, invalid compact offsets and IDs, corrupt dimensions/encodings, and corrupted secondary payload rebuild. The new sparse test compares the fast validator with the previous decoder on deterministic malformed byte sequences. Musenalm direct-source bigram and JSON oracles passed during the compact-manifest step.
- Cache restoration checks the expected base digest in the secondary sidecar. V5 is recognized explicitly; incompatible, corrupt, or missing sidecars are cache misses. Existing checksums, source identity checks, atomic publication, and freshness reconciliation remain in place. The early restore remains provisional until the subsequent source inventory confirms freshness.
- The [100-case JSON run](results/snapshot-restore-final-json100-2026-11-03.ndjson) and [30-case FTS run](results/snapshot-restore-final-fts30-2026-11-03.ndjson) have identical match, first-ID where applicable, scan, read, parse, and candidate signatures to the [pre-final JSON run](results/snapshot-restore-json100-2026-11-03.ndjson) and [pre-final FTS run](results/snapshot-restore-fts30-2026-11-03.ndjson). Summed per-case medians: JSON 4,319.0 ms before versus 4,352.9 ms in a [seven-run repeat](results/snapshot-restore-final-json100-seven-2026-11-03.ndjson) (+0.8%); FTS 564.5 versus 573.6 ms in a [seven-run repeat](results/snapshot-restore-final-fts30-seven-2026-11-03.ndjson) (+1.6%). First three-run final samples varied more: 4,281.4 ms JSON and 621.2 ms FTS. JSON case 40 ranged 64–122 ms within a single run set, explaining an apparent >30% single-case regression; its seven-run median was 69.8 ms versus 65.8 ms before. Query execution code did not change in this work.

## Limits

These are warm-cache measurements. This environment did not provide controlled cold-storage runs, so no cold-I/O claim is made. The early restoration numbers exclude the full source freshness inventory. Rejected experiments are preserved as raw measurements, but their prototype code is reverted. The final compact format intentionally requires cache regeneration when an older application encounters V5; this application continues reading V1–V4 snapshots.
