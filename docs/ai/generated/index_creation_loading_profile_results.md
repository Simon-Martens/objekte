# Index creation and snapshot restoration profile

## Result

On the configured Musenalm corpus (148,486 files, 193.30 MB of source bytes, ASCII bigrams, inference off, 32 workers), **fresh construction took 1,015 ms median** and **snapshot restore took 802 ms median**, without profiling. The first searchable result appeared at 334 ms in the profiled construction runs; full completion was about 1,008 ms. A restored index was usable at the return from `Index::load_cache`; its automatic, unchanged-source reconciliation continued for roughly 345 ms after that return. These are warm filesystem-cache, fresh-process results, not cold disk-start measurements.

Construction's largest elapsed costs are discovery (240 ms), concurrent source/index processing (477 ms, including early publication), and the final segment flush (262 ms). The latter is mostly **index sealing** (241 ms, of which 148 ms is bigram finalization and 83 ms sorting across segments), rather than catalogue lock time. The largest collection, `contents`, accounts for 131,900 records in its final segment and 355 ms of measured ordered admission.

Restore spends about 417 ms in the checked snapshot reader, 53 ms rebuilding the live catalogue, 291 ms decoding and validating eight segments, and 11 ms checking cross-segment IDs. Snapshot payloads are read twice logically: 132.71 MB for checksums and the same 132.71 MB for deserialization. Both passes were served from cache in these runs (`/proc/self/io` charged zero physical `read_bytes`). The evidence points to CPU, allocation, and memory traversal as the main load costs, rather than storage latency. None of the measured full restore is explained by reparsing source documents.

## Scope and measurement setup

The desktop opening path at `crates/desktop/src/backend/open.rs:386-406` uses `Index::load_cache` and falls back to `Index::start_incremental_build`. The CLI TUI path at `crates/cli/src/execution.rs:113-174` does the same for its selected cache/incremental modes. The benchmark therefore measures those two core entry points, including normal checked loading and automatic reconciliation. The older direct `Repository::open_with_options` builder is not included in the primary figures.

- Measurement code: `8d4cfdc4c7ceb6baea08a580b1631c2c30ff6502`, integration branch `integration/index-creation-loading-profiling`; base master `8682dcb201d3cd0b6c1c0bf0b1a5c552f1710b67`. That base includes the intervening TUI completion commit after the task's original `028f62b106` reference.
- CPU: Intel Core i9-13900K; process affinity CPUs 0-31; performance governor. Rust 1.99.1, release profile, locked dependencies. Corpus and snapshot are on Btrfs over NVMe, with zstd:1 compression. The host had other background load (post-campaign load average 8.87/5.54/3.02); no host-wide cache clearing or task isolation was attempted.
- Corpus inventory fingerprint (paths, mtimes, sizes): `9f93a4ba7fb9249c9c4eb04e03ed911435a6b0493f15b087a5447add32456aa9`. Schema-content fingerprint: `4115070eb30b0af98b109429d0883b493f857cdf671ec55b943d91b73c5ebfe3`. These identify the observed inventory/configuration; the inventory fingerprint is not a byte hash of every source file.
- Lifecycle snapshot format version 28; fixture 132,710,084 bytes, SHA-256 `b189e5954dc353828b03318f85bbeb1bce2a847e53c4750dd31d27bb7cfe4e12`. It was created once from this revision outside restore timing. Save call: 353 ms, storage save timer: 193 ms, excluded from all primary totals.
- The benchmark binary SHA-256 is `443e89cd18ddeee98d9e7c77850522bc3b7cd226a1357c1b4791d1bcb7339365`. See [environment.json](../../../bench/results/index-creation-loading-profiling/environment.json), the per-run manifests and the complete commands in [run-primary.sh](../../../bench/results/index-creation-loading-profiling/run-primary.sh) and [run-diagnostics.sh](../../../bench/results/index-creation-loading-profiling/run-diagnostics.sh). Each invocation is a new process; profile order alternates by round. Query digest checks run after the construction interval.

The table reports milliseconds. Quartiles use inclusive 25th/75th percentiles of seven samples. Raw values are in execution order, rounded only for display; exact nanoseconds remain in `sample.json` files.

| Primary operation | Profile | Median | Q1–Q3 | Raw ms |
|---|---:|---:|---:|---|
| Incremental construction | off | 1,015.0 | 995.5–1,057.9 | 1,000.7, 1,021.3, 1,094.5, 1,132.8, 1,015.0, 990.3, 964.9 |
| Incremental construction | on | 1,008.4 | 976.0–1,053.4 | 960.4, 1,077.3, 1,059.4, 1,008.4, 991.5, 957.4, 1,047.3 |
| Lifecycle restore | off | 802.4 | 779.5–806.3 | 779.6, 802.4, 835.5, 775.0, 779.4, 809.1, 803.6 |
| Lifecycle restore | on | 775.4 | 766.7–794.0 | 757.4, 811.9, 775.4, 811.1, 776.9, 774.8, 758.5 |

Profile-on medians were 6.6 ms lower for construction (−0.6%) and 27.0 ms lower for restore (−3.4%). These negative differences show no detectable positive overhead at this sample size; they are not a speedup from instrumentation. The trials vary more than those differences, so the unprofiled medians are the performance baseline and the profiled runs supply attribution.

## Construction timeline: exclusive elapsed intervals

The intervals below form the outer construction path. Each number is a median across the seven profiled runs; medians of separate intervals need not add exactly. Per-run residual is calculated before taking its median. Worker sums, receive waits, and publication substeps are nested and excluded here.

| Outer phase | Median ms | What it includes |
|---|---:|---|
| Pool initialization | 0.9 | Rayon worker pool setup |
| Discovery and inventory | 239.9 | Directory enumeration, metadata, sorted insertion, inventory validation |
| Builder/catalogue setup | 10.9 | Collection setup and catalogue callback |
| Job scheduling | 0.9 | Work-item construction |
| Source and index processing | 477.1 | Parallel workers, coordinator receive/admission, and early publications |
| Final segment flush | 261.7 | Seal, index finalization, publication and callbacks |
| Fingerprints and completion state | 11.6 | Fingerprint finishing, generation/inference summaries, final bookkeeping |
| Unattributed per-run residual | **3.7** | Small call/timer boundaries and final publication |

The median residual is **0.37%** of the profiled operation, so more than 99% of its outer elapsed interval is attributed without adding overlapping work. Discovery itself separates into 59.4 ms enumeration, 87.8 ms metadata preparation and 87.7 ms ordered inventory insertion; its median per-run remainder is 2.2 ms. Catalogue was observed at 252.0 ms, first publication at 333.7 ms, first result at 333.8 ms, full completion at 1,008.4 ms, and caller return at 1,008.4 ms. The first-result probe itself was 0.06 ms median, measured separately. Catalogue readiness depends on discovery and setup; the first query depends on the first sealed/published segment; full completion waits for all work, final segment sealing and fingerprints.

Publication time across the build was 266.9 ms, **nested** in source processing and final flush. Its measured subparts were 241.0 ms segment index construction, 19.6 ms installation and 2.4 ms callbacks. The final flush alone was 261.7 ms; about 4.3 ms of publication occurred earlier. Calling the full 266.9 ms a lock or notification cost would substantially overstate that opportunity.

## Construction work, waiting and collection concentration

These are accumulated worker/component measurements, not additive elapsed percentages. The existing profile samples every 64th physical file, with component timers on up to one record from each sampled file. The seven runs each sampled 2,321/148,486 files and records (1.56%), 3.061 MB of the 193.297 MB source corpus (1.58%), with 4,672 sampled read calls. The close byte coverage supports a rough representativeness check, but file sizes and index complexity vary, so sampled milliseconds below are **not multiplied by 64 and represented as measured wall time**. Batch finalization covers all 294 batches.

| Work counter | Accumulated median ms | Coverage/meaning |
|---|---:|---|
| All worker job execution | 4,683.5 | Concurrent job durations; can exceed wall time |
| Batch finish | 232.7 | All 294 batches; local index packing/finalization |
| Source open / read / hash / framing | 5.0 / 2.5 / 1.4 / 0.2 | Sampled files only; warm-cache read |
| Source extraction, including callback | 61.8 | Sampled files; callback subset 44.3 ms |
| Validation / postings / text normalization and bigram work | 5.4 / 16.6 / 12.2 | Sampled components; text timer combines normalization and gram production |
| Sorting / relation / custom text / inference | 4.0 / 0.8 / 0.6 / 0.1 | Sampled components; inference disabled in baseline |
| Coordinator receive wait | 75.7 | Nested in source processing; can overlap worker execution |
| Ordered admission | 388.1 | Nested in source processing, includes batches awaiting/publication work |

The ready/scheduling counters show substantial overlap and serial pressure: 32 workers were available, worker execution accumulated 4.68 seconds, ordered admission accumulated 388 ms, and coordinator receive calls 75.7 ms. The queue saw job-limit blocking (232 in the first profiled sample), no byte-limit blocks, and a high aggregate completion-to-receipt interval because jobs can finish out of admission order. These aggregate queue intervals overlap one another and cannot be subtracted from wall time. Source processing is CPU-heavy: the construction diagnostic `perf stat` process used 8.14 seconds of task clock for 1.44 seconds of whole-process wall time, including post-build benchmark work. The 1-worker/8-worker/32-worker diagnostic medians were 3,430 / 980 / 1,053 ms respectively (three runs each); the large gain to eight workers and absence of a further gain at 32 point to parallel worker work plus serial admission, sealing, and memory/scheduling contention. This is corroboration, not a fitted scaling law.

| Collection | Sampled files | Ordered admission ms | All-batch finish ms | Sampled bigram ms | Final segment files / seal ms |
|---|---:|---:|---:|---:|---:|
| `contents` | 2,093 | 354.7 | 213.7 | 8.4 | 131,900 / 207.9 (plus an early 2,048-file segment) |
| `agents` | 63 | 7.3 | 6.2 | 3.1 | 4,010 / 11.3 |
| `entries` | 77 | 10.2 | 7.7 | 0.5 | 4,928 / 10.3 |
| `items` | 65 | 2.6 | 3.0 | 0.1 | 4,180 / 3.2 |
| `pages`, `places`, `series` combined | 23 | 2.7 | 1.6 | 0.1 | 1,420 / 2.8 |

The large `contents` segment contains 147.6 MB of source bytes and 131,900 records. Its 207.9 ms seal contains 119.3 ms bigram finalization and 79.6 ms sorting finalization; installation costs another 18.4 ms. Across all eight segments, seal family totals are 147.5 ms bigrams, 83.0 ms sorting, 7.0 ms posting finish/validation and 2.9 ms identity order. The 294 batch finishes add 232.7 ms of parallel worker work before this sealing stage. For source processing, `build_source_reads`, `build_parses` and `processed_files` each equal 148,486 and `build_source_bytes_read` equals 193,297,230.

The Unicode cross-check was 1,060 ms median (three profiled runs) versus 1,008 ms for profiled ASCII baseline, a roughly 51 ms difference with overlapping variation. A single unconfigured/plain run was 709 ms and a single explicit-inference run 2,357 ms. The plain run changes indexing policy and the inference result has no repetition; neither should be used as an ASCII baseline speedup estimate.

## Snapshot restore: exclusive elapsed and nested work

| Outer restore phase | Median ms | Detail |
|---|---:|---|
| Checked storage load | 417.2 | Framing, checksum pass, deserialization, inventory rebuild and validation |
| Supplemental metadata / file versions / base active | 0.2 | Near zero for this fixture |
| Live catalogue reconstruction | 52.8 | Reference resolution, path map and liveness state |
| Eight segment decodes and checks | 290.7 | 104.16 MB of encoded segment buffers |
| Cross-segment ID uniqueness | 11.3 | Active ID set traversal |
| Publication setup and reconciliation scheduling | 1.0 | Index becomes usable at return |
| Unattributed per-run residual | **2.2** | Root/path and call boundaries |

The median residual is **0.28%** of profiled restore, again over 99% outer attribution. Inside the 417.2 ms storage phase, section framing/checksum/deserialization totals 281.1 ms, inventory rebuild 104.4 ms, lifecycle validation 26.6 ms, and remaining configuration/collection/relation/repository work roughly 2 ms. The two large sections are the 21.50 MB base inventory section (46.4 ms) and the 111.14 MB lifecycle section (234.9 ms). Across sections, the checksum pass hashes 132,709,592 bytes in 49.9 ms, with 11.6 ms in buffered reads and 2,033 explicit read calls; deserialization consumes the same logical byte count in 219.6 ms, with 12.1 ms nested buffered reads and 2,033 read calls. Seek time is negligible. These numbers are nested within the section and storage totals.

Segment decode separates approximately 95.8 ms in bincode deserialization, 26.6 ms reconstructing metadata/lookups, 62.5 ms identity/domain checks, 71.9 ms bigram checks, 19.2 ms sorting checks, 2.9 ms posting checks and 1.5 ms relation checks. The 78.79 MB main `contents` segment alone costs 208.8 ms, including 82.0 ms deserialize, 58.5 ms identity/domain checks, 25.3 ms metadata/lookups and 18.7 ms sorting checks. A separate 9.32 MB `agents` segment spends 39.5 ms in bigram validation, showing that validation cost does not simply track encoded bytes. The base inventory, lifecycle envelope, decoded segment structures, live catalogue and segment lookup maps coexist during loading; their bytes and checks are not interchangeable.

The restore process read `rchar` of about 265.42 MB through 4,125 `syscr` calls, matching two logical passes over 132.71 MB plus small overhead. `/proc/self/io` reported zero `read_bytes` for the timed load in all seven profiled runs. That is consistent with a warm page cache, not proof that the source files or snapshot require no physical reads on a cold machine. No snapshot write was included. The checked loader validates checksums, metadata, index domains, identities, sorting and relations; the subsequent catalogue and segment setup rebuild derived path and ID lookups. These are separate work and correctness obligations. Eliminating a traversal would require preserving its invariant through another checked path, not merely deleting a check.

Automatic reconciliation was enabled. All restore trials returned `restored_from_cache=true`, then examined 148,486 files; `planned_changes`, `added`, `updated`, `removed` and `indexed_bytes` were zero. The scan traverses directories and stats files in `crates/core/src/lifecycle_scan.rs:240-335`; actual content indexing occurs only for planned changes in `crates/core/src/lifecycle.rs:2230-2270`. Thus these unchanged runs did not parse source content during reconciliation. The post-return process I/O interval had median `rchar` 101 bytes and zero physical `read_bytes`; some scan work may overlap the measured load, so this counter alone does not isolate its total work. Reconciliation completed at 1,117 ms median from load-call start, with 345 ms median wait after the usable index return. It is a background freshness scan, not part of the 802 ms index-ready time.

## Memory, correctness and profiler limits

Construction finished with 562 MB median RSS and 572 MB process peak; estimated index families were 71.2 MB and inventory heap 58.5 MB. Restore returned with 329 MB RSS and 406 MB process peak. The 104.16 MB encoded segment buffers are a logical live-byte count during restore, not an allocator total; the 77 MB gap between load peak and post-load RSS is consistent with transient overlap but cannot be assigned wholly to those buffers. Snapshot save (excluded from build/load timing) raised that process's peak to 718 MB, with 606 MB RSS after write. A process high-water mark cannot be reset for each phase.

All 28 final primary samples produced the same five query digests, and all 14 restore samples took the restore path. The core profile-on/off regression test passed, as did `cargo test --locked --workspace --all-targets --quiet` (no failures). The release benchmark built with `cargo build --release --locked -p objekte-search-core --example bench_index_construction`. The profile is opt-in and does not change the snapshot format or disable validation.

`perf stat` was available and supports CPU-heavy execution, but `perf record` could not open sampling events under this host's `perf_event_paranoid=2` access policy. No sampled stacks or flamegraph are claimed. The `perf stat` process counters include query checks, reconciliation and result writing after the measured startup intervals, so they cannot be divided into per-phase CPU percentages. Timers are coarse and overlapping worker/queue fields remain explicitly nonexclusive. Warm-cache timing, background host load and seven samples limit small difference estimates. The previous roughly 963 ms build / 758 ms restore figures used another revision and campaign; they are context only, not a matched regression comparison.

## Ranked next candidates (proposals only)

These ranges are hypotheses for a future experiment, not promised savings. Upper bounds refer to observed *elapsed* intervals and assume every byte of that interval disappeared; overlapping worker sums are never used as an upper bound.

| Rank | Candidate and evidence | Possible wall significance and constraint |
|---|---|---|
| 1 | Reduce final segment seal cost at `crates/core/src/mutation_segment.rs:482` and `crates/core/src/bigram.rs:1243`. Sealing is 241 ms, including 148 ms bigram and 83 ms sorting; the large `contents` segment is 208 ms. | Investigate incremental packing, sorting or earlier parallel finalization. A plausible target is tens of ms; **absolute upper bound 241 ms** of the final build path. Earlier sealing may raise peak memory or complicate ordered publication and consistency. |
| 2 | Reuse or construct restore inventory/catalogue maps more directly at `crates/core/src/storage.rs:1619` and `crates/core/src/lifecycle_catalog.rs:97`. Inventory rebuild is 104 ms and catalogue reconstruction 53 ms. | Hypothesis: roughly 15–60 ms if duplicate allocation/hash/path work can be avoided; **upper bound 157 ms**. Must preserve path uniqueness, reference and liveness validation; extra retained maps would trade time for memory. |
| 3 | Examine checked segment reconstruction at `crates/core/src/lifecycle_segment_codec.rs:319` and `crates/core/src/bigram.rs:260`. Decode/check totals 291 ms, with 96 ms deserialization, 72 ms bigram checks, 63 ms identity/domain checks and 19 ms sorting checks. | Hypothesis: roughly 20–80 ms by reducing redundant passes/allocations while keeping every invariant; **upper bound 291 ms**. Validation is required work, and structural changes risk corrupt-cache acceptance or larger transient buffers. |
| 4 | Reduce ordered admission pressure at `crates/core/src/repository_incremental.rs:227` and `crates/core/src/mutation_segment.rs:313`. Global admission is 388 ms, 355 ms in `contents`, inside a 477 ms source-processing interval. | Hypothesis: roughly 20–80 ms if batch merging or scheduling becomes cheaper; **upper bound 477 ms** for that entire elapsed phase, lower in practice because workers overlap admission. Smaller batches can increase publication overhead and memory; larger batches can delay first results. |
| 5 | Test a single checked byte traversal for snapshot sections at `crates/core/src/storage.rs:1138`. Two 132.71 MB logical passes cost 11.6 ms and 12.1 ms in buffered read calls; SHA-256 hashing itself costs 49.9 ms and remains necessary. | A fused streaming checksum/deserializer could save some cached read/seek overhead, likely **single-digit to low tens of ms**, with a strict **~12 ms measured read-call upper bound** for eliminating only the checksum pass's reads (additional allocation effects unmeasured). It must still verify every byte before publishing data and preserve corrupt-cache rejection; a format/loader redesign would add complexity. |

The largest required work remains extraction/indexing and checked reconstruction. Snapshot-buffer lifetime and one-pass validation are future design questions; no save-path rewrite, format change or performance optimization was made in this task.

## Reproduction and raw artifacts

From the integration worktree, run `cargo build --release --locked -p objekte-search-core --example bench_index_construction`, create the fixture using the `--incremental --alphabet ascii --workers 32 --snapshot --check-queries` command in the task specification with `--result-dir bench/results/index-creation-loading-profiling/restore-fixture-final`, then run `bash bench/results/index-creation-loading-profiling/run-primary.sh` and `bash bench/results/index-creation-loading-profiling/run-diagnostics.sh`. `python3 bench/results/index-creation-loading-profiling/summarize.py` regenerates [summary.json](../../../bench/results/index-creation-loading-profiling/summary.json) from final trial samples. Run the campaigns sequentially, after building, without competing benchmark jobs. Every trial directory contains a manifest with the exact command and a sample JSON. The large `index.snapshot` remains local and ignored; use the SHA-256 and fixture command above to reproduce it. The `perf-stat-*-final.log` files contain whole-process CPU counters, and `perf-construction.log` records the sampling denial.
