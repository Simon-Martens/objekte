# Index construction filesystem I/O experiment — 3 November 2026

**Follow-up:** The combined inventory/scan path was subsequently selected for fresh production builds. The decision below records the original experimental recommendation and its consistency concern; the production behavior is documented in [the implementation guide](../../docs/implementation.md).

## Adoption follow-up

Fresh production builds now use combined ID enumeration and Rayon scanning. Checked snapshot reconciliation retains the separate inventory path; rebuilds with existing slots still compare against their stored expected stamps. The benchmark binary still offers `--separate-inventory` as a reference; its default profiles the production path. One fresh alternating pair after this switch produced these build times (ms):

| Profile | Separate reference | Production combined | Index size equal |
|---|---:|---:|---|
| Amazon | 8,504 | 7,335 | Yes |
| NYC | 13,851 | 12,874 | Yes |
| Leipzig | 2,306 | 1,995 | Yes |
| Musenalm | 3,052 | 2,969 | Yes |

The earlier five-pair results below remain the stronger timing evidence; NYC was variable across those pairs. The [adoption measurements](results/index-io-adoption-2026-11-03.ndjson) also include a fresh 100-case JSON and 30-case FTS run. All case result signatures match the prior run. Summed case medians were 4,281 → 4,364 ms for JSON (+1.9%) and 621 → 592 ms for FTS (−4.7%), within the observed run-to-run variation. The pre-open in-place-edit detection gap described below remains an accepted limitation of fresh builds.

## Original experimental decision

**Keep the production synchronous Rayon path.** The benchmark-only combined inventory/scan path saved 9% on Amazon and 9% on Leipzig, but it removes the earlier per-file metadata observation used to detect an in-place edit between enumeration and opening. NYC did not improve reliably. Synchronous `statx` changed complete inventory time by under 1%; `io_uring` `STATX` and the bounded read-only ring were much slower. No experimental I/O backend was installed in normal repository opening, rebuilding, reconciliation, or query execution.

The experiment code and runner remain in `crates/bench/` and behind `index-timings` in the core. All 477 individual records, including setup and verification rows, are in [the raw measurements](results/index-io-benchmark-2026-11-03.ndjson).

## Method and environment

Starting revision: `bac19727ca`. Release build, 32 Rayon workers for full builds, five isolated runs per configuration, alternating profile and backend order. No snapshot save or restore is included in the build timings (`--skip-snapshot`). The four existing FTS profiles and their field settings are exactly those in `run_index_build_profiles.py`. Linux 7.2.13, Rust 1.99.1, Intel Core i9-13900K, 32 logical CPUs, local Btrfs. Filesystem data was largely warm; caches were not evicted. Current full-build times were materially higher than the earlier second-stage report, so conclusions use same-session paired runs rather than historical absolute times.

To repeat a phase:

```sh
python3 crates/bench/run_index_io_benchmarks.py --phase build --runs 5 --output target/io-build.ndjson
python3 crates/bench/run_index_io_benchmarks.py --phase metadata --runs 5 --output target/io-metadata.ndjson
python3 crates/bench/run_index_io_benchmarks.py --phase read --runs 5 --output target/io-read.ndjson
```

The metadata and read-only benchmark uses the same sorted file IDs and pinned directory descriptors for relative `statx` and open requests. It is a benchmark clone, not a substitute for `Source::inventory()`: it constructs its own in-memory inventory, including a `BTreeMap`, so its absolute phase sum is not the production inventory duration. Comparisons *within* that benchmark use identical enumeration and stamp construction. File-operation counts below are logical operations; `read_to_end` can issue more than one kernel read per file. CPU figures are cumulative worker CPU time; they are not wall time.

## Phase 0: unchanged build and filesystem costs

The production baseline's five-run medians, with the feature-gated detailed scan timers, were:

| Profile | Files | Bytes read | Inventory wall | Parallel scan wall | Total build wall | Peak RSS |
|---|---:|---:|---:|---:|---:|---:|
| Amazon FTS2 | 1,000,000 | 513 MB | 1,312 ms | 6,333 ms | 8,385 ms | 3,619 MiB |
| NYC FTS3 | 1,000,000 | 1,174 MB | 1,236 ms | 11,042 ms | 12,706 ms | 1,943 MiB |
| Leipzig FTS1 | 400,000 | 63 MB | 526 ms | 1,280 ms | 2,187 ms | 874 MiB |
| Musenalm FTS4 | 133,948 | 150 MB | 181 ms | 2,316 ms | 3,166 ms | 772 MiB |

The scan performs one secure open, two descriptor metadata checks, at least one logical read, and one close per file: 1,000,000 opens, 2,000,000 metadata checks, and 1,000,000 closes on Amazon or NYC. The separate inventory performs another 1,000,000 per-file metadata lookups. These are logical calls, not measured kernel syscall totals.

Summed worker CPU medians reveal where scan time is spent (seconds):

| Profile | Open | Before metadata | Read bytes | After metadata | Close | JSON parse | General projection | Field text | Bigram insertion* |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Amazon | 16.9 | 9.2 | 2.4 | 9.1 | 1.0 | 7.0 | 76.4 | 30.0 | 15.4 |
| NYC | 17.9 | 9.8 | 2.5 | 9.7 | 1.2 | 10.2 | 208.1 | 35.3 | 13.1 |
| Leipzig | 5.9 | 3.1 | 0.8 | 3.1 | 0.3 | 2.3 | 2.8 | 8.2 | 5.3 |
| Musenalm | 2.3 | 1.2 | 0.3 | 1.2 | 0.1 | 1.3 | 26.0 | 12.8 | 2.9 |

`*` Main, global rendered, and field insertion CPU summed. These stages can overlap in wall time. The file-size buckets (0–255, 256–1,023, 1,024–4,095, 4,096–16,383, 16,384+ bytes) are recorded for each build in the raw data; for example Amazon has 185,886 / 734,225 / 77,182 / 2,694 / 13 files in those buckets. The inventory clone's Amazon median was 122 ms enumeration, 398 ms name/ID work, 9 ms sorting, 90 ms Rayon metadata acquisition, and 263 ms stamp conversion plus map construction. Thus its metadata lookup is a small share of inventory *wall time*, although its summed worker CPU is about 2 seconds.

## Experiment A: combined inventory and scan

The `index-timings`-only `FilePicker::profile_collection_combined()` enumerates validated IDs, sorts them, creates stable slots, and captures each file's stamp from the securely opened descriptor during the existing scan. It keeps the post-read metadata check, `O_NOFOLLOW`, directory pinning, 64-slot posting ownership, one JSON parse, and final directory validation. Normal builds continue using `Source::inventory()`.

Five alternating pairs, medians:

| Profile | Backend | Inventory | Scan | Total build | Peak RSS | Pairwise combined − reference, ms |
|---|---|---:|---:|---:|---:|---|
| Amazon | Rayon reference | 1,293 ms | 6,008 ms | 8,005 ms | 3,623 MiB | |
| Amazon | Combined | 616 ms | 6,069 ms | 7,281 ms | 3,504 MiB | −506, −841, −592, −1,047, −1,448 |
| NYC | Rayon reference | 1,224 ms | 10,396 ms | 12,033 ms | 1,942 MiB | |
| NYC | Combined | 603 ms | 11,488 ms | 12,456 ms | 1,866 MiB | −242, +479, −1,625, +888, −1,046 |
| Leipzig | Rayon reference | 517 ms | 1,262 ms | 2,139 ms | 888 MiB | |
| Leipzig | Combined | 247 ms | 1,350 ms | 1,954 ms | 814 MiB | −195, −176, −189, −111, −492 |
| Musenalm | Rayon reference | 176 ms | 2,234 ms | 3,075 ms | 773 MiB | |
| Musenalm | Combined | 83 ms | 2,145 ms | 2,930 ms | 746 MiB | −173, −401, −93, +2, −113 |

The gain comes from removing the independent per-file metadata pass and its stamp/map work; it is not an I/O concurrency improvement. The work partly moves into the scan. NYC's scan increase and wide paired spread make its end-to-end result inconclusive. Every paired build had the same file count and compressed index size as its reference. A separate untimed verification built both variants for all four profiles and compared selected literal and regex candidate IDs and complete selective regex match sets; all matched. A feature-gated test also compares ASCII and UTF-8 indexes, malformed JSON, and snapshot restoration.

**Consistency tradeoff:** The reference inventory records a file stamp *before* scanning and rejects a file whose descriptor stamp differs at open. The combined path has only the descriptor's first stamp. It detects modification during reading through its before/after check, and secure descriptor-relative opening still rejects symlinks. It cannot detect an in-place edit that finishes after ID enumeration but before opening; a test demonstrates this. Membership and directory changes rely on directory stamp validation, whose timestamp granularity can coalesce rapid changes. This benchmark-only path is therefore **rejected for production** despite the Amazon and Leipzig speedups.

## Experiment B: metadata backends

`STATX_BASIC_STATS` was requested with no-follow flags relative to pinned directory descriptors. One verification run for each backend and collection compared every returned size, modification time, change time, inode, and device component with `symlink_metadata()`; all entries and ordered fingerprints matched. No inaccessible file was silently skipped. The ring uses bounded, stable request storage and associates out-of-order completions with their original slots.

Five-run median metadata phase and complete clone-inventory times at 32 Rayon workers or ring depth 32:

| Profile | Rayon metadata | Sync `statx` | Ring `STATX` | Complete Rayon inventory | Complete sync inventory | Complete ring inventory |
|---|---:|---:|---:|---:|---:|---:|
| Amazon | 90 ms | 86 ms | 627 ms | 887 ms | 883 ms | 1,418 ms |
| NYC | 91 ms | 87 ms | 615 ms | 871 ms | 870 ms | 1,399 ms |
| Leipzig | 38 ms | 37 ms | 262 ms | 363 ms | 360 ms | 583 ms |
| Musenalm | 13 ms | 12 ms | 89 ms | 128 ms | 127 ms | 198 ms |

The 8/16/32-worker and depth-32/64/128/256/512 sweep is in the raw data. On Amazon, Rayon metadata takes 154/107/90 ms as workers increase; sync `statx` takes 141/101/86 ms. Ring `STATX` takes 627/646/659/659/670 ms across the five depths. NYC behaves similarly. The ring's process CPU is about 2.5 s for a million metadata requests versus about 2.0 s for Rayon; increasing depth does not help. Because sync `statx` saves only about 4 ms in complete clone inventory and ring `STATX` loses about 0.5 s, neither backend passed the gate for full-build integration. No full-build time is claimed for these unintegrated backends.

## Experiment C: bounded read-only ring

The read-only comparison securely opens every file relative to a pinned directory, checks regular-file metadata before and after reading, reads all bytes, and closes the descriptor. The ring submits bounded open/read requests, handles out-of-order completions and short reads, and keeps each buffer owned until completion. Unexpected errors after submission terminate the benchmark process so an in-flight kernel write cannot target freed memory. The synchronous variant reuses one read buffer per Rayon worker. Neither variant parses or indexes JSON.

| Profile | Rayon 32 workers | Best ring depth | Ring read time | Bytes read | Peak RSS, Rayon / ring |
|---|---:|---:|---:|---:|---:|
| Amazon | 394 ms | 256 | 1,847 ms | 513 MB | 309 / 309 MiB |
| NYC | 391 ms | 16 | 1,911 ms | 1,174 MB | 294 / 294 MiB |
| Leipzig | 158 ms | 256 | 673 ms | 63 MB | 126 / 126 MiB |
| Musenalm | 54 ms | 16 | 248 ms | 150 MB | 48 / 48 MiB |

Depths 16/32/64/128/256 and 8/16/32 Rayon workers were run five times each. Amazon's ring read times were 1,849/1,880/1,886/1,890/1,847 ms; NYC's were 1,911/1,927/1,950/1,965/1,974 ms. Amazon read throughput was about 2.5 million files/s with Rayon versus 0.54 million files/s with the best ring. Process CPU for the ring coordinator was approximately equal to its wall time, indicating that this prototype did not spend most of its time waiting for storage. This result is specific to the coordinator and warm-cache workload; it does not prove every possible `io_uring` design is slower. Since the read-only gate failed by more than 4×, no parse/index pipeline was built and no full-build ring-reader time is claimed.

## Verification and limits

- `cargo test --workspace -j 4 --quiet` and `cargo test -p objekte-core --features index-timings --quiet` passed. Workspace Clippy with all targets, the timing feature, and `-D warnings` passed. Changed Rust files pass the repository's rustfmt and blank-line convention.
- Tests cover ID and candidate equivalence, ASCII/UTF-8, malformed JSON, snapshot restoration, membership changes, symlink substitution, recursive directory replacement, and the known in-place-edit gap. The selected full-build verification on all four corpora compared complete selective regex results. Production queries and source reconciliation were not changed.
- The benchmark clone and production `Source::inventory()` have different in-memory representations, so only the full-build paired results are used for end-to-end claims. The ring reader uses synchronous descriptor metadata checks in its coordinator; this may limit overlap, but it preserves the same read-verification steps. The read-only benchmark does not include JSON parsing. No controlled cold-storage run was available, and no cold-cache claim is made.
- Additional ring queue-occupancy and coordinator wait histograms were not recorded; process CPU versus wall and the complete depth sweep were sufficient to reject this prototype. The benchmark reports logical filesystem operations, not exact kernel syscall counts or cache misses.
