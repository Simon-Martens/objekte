# Candidate scanning I/O experiment — 2026-11-03

## Decision

**Retain the production Rayon candidate verifier.** The benchmark-only synchronous coordinator, Compio reader, and direct `io_uring` reader were **rejected for production use**. The completion readers could match or beat Rayon on some 32-file read-only samples, but the largest observed median saving was about 0.06 ms and the seven-run ranges overlap. At 256–4,096 files, Rayon was faster on all four repositories. The larger batches are common in the 100-case JSON and 30-case FTS workloads. No query backend, snapshot, or source-reading policy was changed.

The specified early gate ended this experiment after isolated candidate reads. It did **not** establish full-query parity for an asynchronous pipeline. The small-batch result is therefore **inconclusive** as a possible niche optimization, and insufficient for a production reader with a second I/O coordinator and new cancellation paths.

## Reproduction and environment

- Starting revision: `164d13277f091b62ff960bfc1e2af277bc631ac4`; benchmark source changes were uncommitted during measurement.
- Rust 1.99.1, Linux 7.2.13, Intel Core i9-13900K (32 logical CPUs), SK Hynix NVMe, Btrfs with zstd:1 compression.
- Compio 0.19.2 with its `io-uring` feature; direct `io-uring` crate 0.7.15.
- Release builds, largely warm filesystem cache, no cache eviction. Cold-storage behavior was not measured.
- The picker was built and warmed **outside** the timed reader operation. Runtime/ring initialization was also outside timed reads; initialization duration is recorded separately. Backend order reversed every other round.
- Seven paired rounds for the main sizes (32, 256, 1,024, 4,096) on all four repositories with 16 Rayon workers, an eight-worker synchronous I/O pool, one Compio runtime thread, and one direct ring coordinator. Depths 8 and 32 were measured; tables below use depth 32. Four-worker primary comparisons also have seven rounds. The 8/32-worker and 1/8/16/32/64/128/256-depth sweeps have three rounds. Sizes 1, 8, 128, 16,384 and 65,536 were sampled once to check scaling, not used for statistical claims.
- Full raw records: [results/candidate-io-benchmark-2026-11-03.ndjson](results/candidate-io-benchmark-2026-11-03.ndjson). The experimental runner and I/O backends were removed after the Rayon decision. Allocation runs are identified separately in the raw data and excluded from latency medians.

The four collections were Amazon Reviews (1,000,000 files), NYC 311 (1,000,000), Leipzig multilingual (400,000), and Musenalm contents (133,948). Candidate IDs came from `FilePicker::candidate_ids`, after a normal or UTF-8 byte bigram build. The selective literal was `excellent`, `water`, `Liebe`, or `Goethe`, respectively. Adjacent and scattered samples were reproducible subsets of the broad index candidate list. The selective source candidate pools were 49,744, 147,287, 1,738, and 461 files respectively; samples larger than those pools contain only the available candidates. All samples retain index ID order. This experiment did not construct indexed-sort-order samples or field-specific rendered FTS candidates because the early gate gave no broad-batch win.

### Equivalent read work

All four readers secure-open each selected file relative to a pinned source directory with `O_NOFOLLOW`, compare the expected size/device/inode/mtime/ctime, read the complete file, check descriptor metadata again, and preserve candidate sequence. The read-only benchmark reimplements these operations rather than calling the private `Repository::read_verified_into()` helper. It does not model that helper's retry/optimistic-missing behavior. For every sample, an untimed verification pass compared the ordered byte lengths and content fingerprints across readers. Timed passes verify lengths and stamps. The direct ring keeps stable, initialized owned buffers until all CQEs finish; its error path aborts the benchmark process if a submitted request cannot be drained. Compio owns transferred descriptors on its runtime thread and drains all outstanding futures before reporting an error. Neither backend uses path-based `File::open` for source documents.

## Isolated read-only medians

Milliseconds, scattered IDs, 16 Rayon workers, depth 32 for completion readers. Each cell is the median of seven paired runs. These numbers include secure opening, two descriptor metadata checks, reading and closing, plus reader dispatch; they exclude candidate selection, parsing, matching, hashes, page materialization, and runtime initialization.

| Collection | Files read | Rayon | Synchronous I/O pool | Compio | Direct ring |
|---|---:|---:|---:|---:|---:|
| Amazon | 32 | 0.106 | 0.252 | 0.074 | **0.045** |
| Amazon | 256 | **0.181** | 0.256 | 0.521 | 0.339 |
| Amazon | 1,024 | **0.730** | 0.765 | 2.122 | 1.406 |
| Amazon | 4,096 | **1.941** | 2.176 | 8.947 | 5.981 |
| NYC | 32 | 0.082 | 0.168 | 0.074 | **0.046** |
| NYC | 256 | **0.191** | 0.299 | 0.607 | 0.720 |
| NYC | 1,024 | **0.631** | 0.778 | 2.314 | 1.427 |
| NYC | 4,096 | **2.453** | 2.501 | 9.687 | 6.234 |
| Leipzig | 32 | 0.092 | 0.304 | **0.082** | 0.102 |
| Leipzig | 256 | **0.275** | 0.451 | 0.754 | 0.478 |
| Leipzig | 1,024 | **0.588** | 0.767 | 2.331 | 1.425 |
| Leipzig | 4,096 | 2.503 | **2.216** | 9.070 | 5.902 |
| Musenalm | 32 | **0.086** | 0.276 | 0.089 | 0.116 |
| Musenalm | 256 | **0.217** | 0.217 | 0.817 | 0.532 |
| Musenalm | 1,024 | **0.583** | 0.767 | 2.231 | 1.463 |
| Musenalm | 4,096 | 2.518 | **2.373** | 9.678 | 6.489 |

For scattered Amazon 32-file reads, Rayon ranged 0.039–0.167 ms and the direct ring 0.042–0.046 ms; approximate interpolated p95 values from seven runs were 0.156 and 0.046 ms. NYC ranged 0.044–0.210 versus 0.042–0.097 ms (p95 0.188 versus 0.084 ms). Seven observations make these tail estimates unstable. At 4,096 files, Amazon's ranges were 1.908–2.426 ms for Rayon and 5.880–6.217 ms for the ring; NYC's were 2.052–3.687 versus 5.989–6.698 ms. Compio was consistently slower at broad sizes. The same pattern appears in adjacent and selective samples in the raw records, with some small-batch wins varying by file size and cache state.

For the 4,096-file scattered samples, actual bytes read were 2.11 MB (Amazon), 4.83 MB (NYC), 0.65 MB (Leipzig), and 4.58 MB (Musenalm). File-size bucket counts for 0–255 / 256–1,023 / 1,024–4,095 / 4,096–16,383 / 16,384+ bytes are 738/3026/323/9/0, 0/502/3594/0/0, 3575/521/0/0/0, and 0/1545/2551/0/0 respectively. Actual bytes, CPU time, process high-water RSS, sample origin, and ordered candidate counts accompany each raw row. RSS includes the warmed index and is not an isolated per-backend memory measurement.

### Allocation sample

The separately compiled counting-allocator build measured one scattered sample per size on Amazon and NYC. Values are allocation count / cumulative allocated bytes / peak additional live bytes during the timed reader call. They are diagnostic one-run values, not latency samples.

| Collection, files | Rayon | Synchronous I/O pool | Compio | Direct ring |
|---|---:|---:|---:|---:|
| Amazon, 32 | 193 / 19.7 KB / 4.1 KB | 178 / 20.5 KB / 4.1 KB | 231 / 30.0 KB / 23.7 KB | 98 / 2.2 KB / 1.0 KB |
| Amazon, 4,096 | 18,235 / 1.93 MB / 262 KB | 14,467 / 1.23 MB / 196 KB | 28,679 / 3.56 MB / 121 KB | 12,417 / 0.28 MB / 66 KB |
| NYC, 32 | 193 / 42.7 KB / 10.4 KB | 182 / 43.3 KB / 10.1 KB | 231 / 30.0 KB / 23.7 KB | 98 / 2.2 KB / 1.0 KB |
| NYC, 4,096 | 19,122 / 3.76 MB / 300 KB | 14,781 / 1.84 MB / 202 KB | 28,679 / 3.57 MB / 121 KB | 12,417 / 0.29 MB / 66 KB |

The ring's reusable initialized buffers reduce temporary allocation volume, but its 4,096-file wall time remains about 2.4–3.1× Rayon. Lower allocation volume alone is not an adoption reason. Process CPU time in the raw rows includes active worker threads, not only coordinator CPU; worker and wall times must not be added together.

## Production query control and stopping decision

The existing 100-case Musenalm JSON workload and 30-case FTS workload were run three times on the unchanged query backend. Result records and source-work metrics are appended to the raw file with `phase=production-query-control`. Summed per-case medians were 4,120 ms and 603 ms, respectively. These are **control timings, not asynchronous end-to-end timings**.

- JSON: 46/100 cases read zero to eight source files, mostly indexed page verification; 42/100 read more than 256. Only 12 cases read 9–256 files. The small read-only ring advantage cannot help queries that avoid candidate verification or read just one returned record.
- FTS: 23/30 cases scanned more than 256 candidates, where Rayon wins. Four scanned 33–256 and one scanned 9–32. A 39-candidate raw-regex case took 0.422 ms end to end, leaving little room for a reader-only saving of a few hundredths of a millisecond after introducing an I/O pipeline.
- The synchronous I/O coordinator did not provide a broad benefit. Because this phase does no parsing, it is only the read stage of a possible synchronous pipeline; it does not establish CPU/I/O overlap. Its persistent pool and channel are included to control for the additional coordinator cost.

The late-stage 8/32-worker and queue-depth sweeps did not reverse the broad-batch result; their individual observations are in the raw data. One- and eight-file samples mostly measure dispatch overhead. The 16,384- and 65,536-file samples have one repetition and are scaling checks only.

**Gate outcome:** Compio and the direct ring lose decisively on the candidate classes that dominate counted and broad queries. The ring's small-batch read-only advantage overlaps Rayon variability and is too small to justify the full semantic integration and safety surface. Thus the experimental backends stop here. No benchmark-only JSON/FTS parser was presented as equivalent to production, and no complete asynchronous query-result comparison is claimed.

## Correctness, security, and limits

- The benchmark's unit tests check identical content across all four readers, detection of a changed file before opening, symlink substitution rejection, and source-directory replacement detection. Every performance sample also compared ordered byte lengths and untimed content fingerprints.
- The existing JSON source oracle and Musenalm bigram oracle passed in release mode with `--ignored --nocapture` (23.67 s and 118.32 s respectively). The ordinary core oracle test also passed. The production JSON and FTS control workloads completed against the unchanged backend.
- The read-only benchmark does not exercise JSON parsing, rendered Markdown/HTML/XML, streaming callbacks, pagination, optimistic missing-file behavior, source retry policy, or final record hashes. Those would require a complete query pipeline and are a condition of any future adoption. The present result makes that investment unjustified.
- No controlled cold-storage benchmark was available. A separate cold-storage device could change the ranking; it must be measured before claiming a cold-storage advantage.
- The 32-file effect merits reconsideration only if a realistic end-to-end query profile shows a repeated gain after all coordination, parsing, matching, page verification, and runtime costs.

Historical verification included the experimental binary's tests, the JSON and Musenalm source oracles, the full workspace suite, and warning-free Clippy. The experimental binary and its dependencies were subsequently removed; current checks use the remaining core and workspace targets.
