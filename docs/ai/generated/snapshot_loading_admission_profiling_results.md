# Snapshot loading and incremental admission profile

Task `snapshot-loading-admission-profiling`; integrated result on `integration/snapshot-loading-admission-profiling`. The matched, profile-off, warm-cache Musenalm restore median fell from **817.05 ms (B0)** to **683.92 ms (B3)**, a 133.13 ms or 16.3% reduction. Inventory construction once (B1) and the proven bigram-domain shortcut (B2) each improved the independent variant. Construction stayed within run-to-run variation. In seven B3 profiled builds, exclusive admission work was 395.21 ms median, dominated by field postings (277.09 ms) and record inventory (81.73 ms); the unexplained residual was 0.13–0.20% per run.

These numbers are fresh-process, warm-page-cache observations, with other host activity possible. They are not cold-start or CPU-cycle claims. [Raw samples and reproduction instructions](../../../bench/results/snapshot-loading-admission-profiling/README.md), [machine and fixture hashes](../../../bench/results/snapshot-loading-admission-profiling/environment.json), and [recomputed summary](../../../bench/results/snapshot-loading-admission-profiling/summary.json) accompany this report.

## Variants and method

| Variant | Revision | Meaning |
|---|---|---|
| B0 | `6c808ebb70203d2442a5e8f2f595199ae5f8d171` | Prerequisite profiling code, before these changes |
| B1 | `7ac10f36cd762708aa943a70fa4700ffb9592d5c` | Restore inventory maps once |
| B2 | `55a38262b28af74cb5858c4f4478385ff4b15804` | Proven full-domain bigram shortcut, isolated from B1 |
| B3 | `93a98cc4dc578dde768c3a83e41b97d36f33e9b2` | B1+B2 plus admission instrumentation |

Release binaries were built at those exact commits with `cargo build --release --locked -p objekte-search-core --example bench_index_construction`. `trial-meta.json` records the true executable revision and SHA-256; the benchmark's built-in `run-manifest.json.code_revision` identifies the B3 runner checkout for copied B0/B1/B2 binaries and must not be used as their source revision. The final branch adds a later tests-only commit `794b00cd2e`, after the release build. No production source changed between B3 and that commit.

All variants used the configured Musenalm workload: 148,486 files/records, 193,297,230 source bytes, inventory fingerprint `9f93a4ba7fb9249c9c4eb04e03ed911435a6b0493f15b087a5447add32456aa9`, schema fingerprint `4115070eb30b0af98b109429d0883b493f857cdf671ec55b943d91b73c5ebfe3`, ASCII bigrams, inference disabled, 32 workers, and fixed affinity CPUs 0–31. The machine was an Intel i9-13900K with performance governor, Fedora kernel 7.2.15, Rust 1.99.1 and Btrfs/zstd on NVMe. No host-wide cache clearing was done. Each trial launched a new process. Four restore variants were rotated across seven rounds; B0/B3 constructions and B3 profiled constructions were interleaved. Profiled restore phases used three trials per variant. The query checks ran after the measured load/build interval.

The B0-created ASCII lifecycle-format-28 fixture is 132,710,084 bytes, SHA-256 `2a12d69cdc64a3e78e0af719138f5c2a445615f970d6cffb7f17623716b32ee2`. Every primary restore read that **same** snapshot. The B0-created Unicode fixture is 135,240,992 bytes, SHA-256 `f5ced9b1c9af5f85a8fa43cafef7236a6024299c1bfa0c323076e7ee30ed5d7f`. Snapshot construction and saving were outside restore timing. Both large fixtures and the release binaries remain local and ignored; their hashes, generation commands, and exact trial commands are archived.

Tables use milliseconds unless noted. Q1/Q3 are inclusive quartiles. Medians of separate components do not generally add to the median of a whole interval; the implementation and summary script reconcile each **individual run** before aggregating.

## Matched restore results

| Profile-off restore | N | Median | Q1–Q3 | Execution-order raw samples |
|---|---:|---:|---:|---|
| B0 | 7 | 817.05 | 801.11–823.74 | 806.42, 817.05, 826.29, 780.05, 795.80, 821.20, 862.59 |
| B1 | 7 | 716.56 | 712.70–729.67 | 740.66, 710.98, 718.67, 716.56, 707.25, 805.82, 714.43 |
| B2 | 7 | 722.77 | 698.85–751.91 | 866.65, 685.14, 689.96, 707.74, 722.77, 736.96, 766.87 |
| B3 | 7 | 683.92 | 662.62–691.15 | 677.14, 642.94, 693.13, 689.17, 648.10, 734.18, 683.92 |

Paired per-round `variant minus B0` restore differences had medians **−88.55 ms** (Q1–Q3 −106.84 to −64.63) for B1, **−84.25 ms** (−113.81 to −72.67) for B2, and **−133.15 ms** (−160.91 to −110.08) for B3. These are paired comparisons under rotated ordering. The individual effects are not additive: the combined median improvement is smaller than their sum because phases interact and elapsed time includes allocation, cache and scheduling variation. Paired raw B1 differences were −65.76, −106.07, −107.61, −63.49, −88.55, −15.38, −148.16 ms; B2 differences were +60.23, −131.91, −136.32, −72.31, −73.03, −84.25, −95.72 ms; B3 differences were −129.28, −174.11, −133.15, −90.88, −147.70, −87.02, −178.67 ms. The B2 first-round 866.65 ms outlier is retained in the raw record. The repeated medians and phase timings support retaining both changes.

| Profiled restore phase | B0 median | B1 median | B2 median | B3 median | Interpretation |
|---|---:|---:|---:|---:|---|
| Full load | 796.86 | 744.30 | 704.37 | 647.74 | Three profile-on runs each; diagnostic, not the primary latency comparison |
| Checked storage load | 416.91 | 374.06 | 419.97 | 369.68 | Includes inventory rebuild |
| Inventory reconstruction | 103.97 | 55.33 | 102.86 | 55.94 | B1 removes repeated map construction; about 48 ms of this phase |
| Segment decode/check | 292.78 | 290.23 | 217.28 | 214.72 | B2 affects validation inside decode |
| Bigram validation within decode | 72.59 | 73.19 | 0.52 | 0.49 | B2 removes redundant full-domain subset traversal in this fixture |
| Live catalogue reconstruction | 51.54 | 53.01 | 51.89 | 50.62 | Separate work, not changed by B1 |

In every B2/B3 profiled ASCII restore, counters show **9** proven logical-domain scans skipped, **0** mixed-domain logical scans, and **8** retained fallback-domain checks. The fixture happens to have all-logical segments. Mixed logical/fallback and corrupt-domain fixtures in tests exercise retained scans and rejection. The 72 ms bigram validation reduction is a phase observation for this corpus, not an assertion that every possible segment takes this path.

The three Unicode profile-off cross-checks had B0 median 799.54 ms (813.82, 798.41, 799.54) and B3 median 690.21 ms (690.21, 658.46, 698.47). A profiled Unicode B3 restore also showed 9 logical-domain skips and 0 logical-domain scans with the same query digests. This supports alphabet independence, with only three timing pairs.

## Construction, readiness and memory

| Incremental construction | Profile | N | Median | Q1–Q3 | Raw ms |
|---|---|---:|---:|---:|---|
| B0 | off | 7 | 985.63 | 975.07–1011.49 | 1116.12, 933.33, 985.63, 983.26, 966.89, 1021.07, 1001.90 |
| B3 | off | 7 | 979.25 | 968.00–1001.06 | 944.18, 980.92, 1024.64, 960.98, 1021.21, 979.25, 975.02 |
| B3 | on | 7 | 972.84 | 951.68–1028.97 | 1041.03, 952.64, 1016.91, 972.84, 950.72, 1049.41, 943.80 |

The paired B3-minus-B0 profile-off construction median was −22.27 ms (Q1–Q3 −34.35 to +43.30), too variable to claim a construction speedup. B3 profile-on versus profile-off medians differ by −6.41 ms; there is no detectable positive overhead from this opt-in instrumentation at seven runs, though that difference itself is noise rather than a speedup. B0/B3 profile-off catalogue observations were 221.98/210.06 ms median, first publications 308.73/293.62 ms, and full completions 985.63/979.25 ms. B3 profiled first publication was 297.71 ms and first query result 297.74 ms; the early query path remains available.

| Profile-off memory | B0 median | B3 median | Unit |
|---|---:|---:|---|
| Post-load RSS | 330.15 | 327.70 | decimal MB |
| Load-process peak RSS | 407.89 | 405.37 | decimal MB |
| Post-build RSS | 559.73 | 559.55 | decimal MB |
| Build-process peak RSS | 570.24 | 569.06 | decimal MB |

The B1-only post-load RSS was 327.69 MB and B2-only 329.90 MB. No repeatable memory growth appears. These are process RSS and high-water marks, not a breakdown of live structures versus allocator retention. Snapshot format and bytes are unchanged: all primary variants restored the exact B0 file; no extra retained lookup map was added.

Every profile-off restore had `restored_from_cache=true`, 0 source-content reads and 0 parses. All 68 trial sample sets yielded the same five deterministic query digests across checked queries; the fixture generation used the same checks. For B0/B3 profile-off restores, the automatic reconciliation examined 148,486 files and marked all unchanged, with 0 planned changes, indexed bytes, adds, updates or removals. It finished at median 1168.49/1034.25 ms from load-call start, respectively. The post-return wait medians were 351.37/345.68 ms. These are background freshness scans; index readiness occurs at load return. The reconciliation interval can overlap loading, so it must not be added as an independent phase.

All four ASCII restore variants had median `/proc/self/io` logical `rchar` about 265.42 MB and about 4,124–4,125 read syscalls, consistent with two logical passes over the 132.71 MB snapshot plus overhead. Physical `read_bytes` was 0 in the measured intervals on this warm-cache host. Construction B0/B3 each had about 193.30 MB logical `rchar`, 297,007 read syscalls and zero physical `read_bytes`; each read and parsed 148,486 source files. These counters do not predict cold-storage latency.

## Exclusive incremental admission

The new profile measures all 294 incremental batches, 148,486 file memberships and 148,486 records. It times coordinator fingerprint/counter bookkeeping and the actual eager segment merge per batch, including ordinary and framed large-file/chunk callbacks. It ends before sealing, publication installation/callbacks and profile aggregation. Consumed local contribution temporaries are dropped inside those boundaries. Profile-off paths create no component profile and read no clocks in the admission loop. [`AdmissionBreakdown`](../../../crates/core/src/build.rs#L2434) is exported globally and by collection in benchmark JSON. The older `build_admission_ns` retains its broader scheduling callback boundary; its median was 399.01 ms, while new exclusive `admission.total_ns` was 395.21 ms. Per-run old-minus-new differences ranged 3.34–5.05 ms, mostly publication and callback/outer boundary work, and are not silently assigned to merging.

| Additive exclusive component | B3 profiled median ms | Scope |
|---|---:|---|
| Coordinator bookkeeping | 6.793 | Fingerprints, counters, observation/capacity control |
| File/record inventory | 81.732 | Files, ranges, slot rebasing, diagnostics and ID lookup |
| Field postings | 277.086 | Field posting merges, including embedded structural facts |
| Custom postings | 0.018 | Custom operation posting merge |
| Inference | 0.016 | Disabled mode still has measured branch cost |
| Relations | 12.560 | Relation contribution merge |
| Raw-text bigrams | 13.020 | Raw text contribution merge |
| Fallback-text bigrams | 0.017 | Fallback contribution merge |
| Custom-text bigrams | 0.114 | Custom text contribution merge |
| Segment structural bitmaps | 1.851 | Logical/empty-array bitmap appends; field facts excluded |
| Sorting contributions | 9.978 | Key append, excluding final sort |
| Unattributed residual | 0.635 | Timer and small boundary work |
| **Measured exclusive admission work** | **395.206** | Sum of components plus residual **per run** |

`bigrams_ns` is a **nonadditive** parent of raw/fallback/custom bigram fields (median 13.150 ms). The `callback_ns` field in the admission structure is unsupported for this incremental path; its serialized zero is **unavailable**, not a measured zero callback. Publication callbacks are measured separately. Per-run admission residuals were **0.167%, 0.201%, 0.141%, 0.146%, 0.145%, 0.141%, 0.128%**. The summary script asserts exact integer-nanosecond reconciliation on every run using the exclusive fields and residual, with checked subtraction in Rust so negative overlap fails rather than clamping. The highest unexplained fraction is far below the 5% target.

| Collection | Batches | Files / records | Admission ms | Field postings ms | Inventory ms | Raw bigrams ms | Relations ms | Sorting append ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `contents` | 262 | 133,948 / 133,948 | 368.982 | 258.026 | 77.501 | 12.004 | 12.179 | 9.696 |
| `agents` | 8 | 4,010 / 4,010 | 11.666 | 8.710 | 1.872 | 0.329 | 0.001 | 0.140 |
| `entries` | 10 | 4,928 / 4,928 | 10.062 | 7.989 | 1.134 | 0.380 | 0.228 | 0.099 |
| `items` | 9 | 4,180 / 4,180 | 2.777 | 1.162 | 1.160 | 0.217 | <0.001 | <0.001 |
| `pages` | 1 | 8 / 8 | 0.014 | 0.004 | 0.003 | 0.001 | <0.001 | <0.001 |
| `places` | 1 | 210 / 210 | 0.123 | 0.067 | 0.030 | 0.005 | <0.001 | 0.002 |
| `series` | 3 | 1,202 / 1,202 | 2.467 | 1.770 | 0.385 | 0.092 | 0.137 | 0.009 |

The dominant `contents` collection accounts for about 369/395 ms of exclusive admission. Its coordinator median is 6.027 ms and residual median 0.391 ms. Collection medians need not sum to the global median. Accumulated worker job execution was 4673.38 ms median and coordinator receive wait 67.99 ms; these overlap parallel workers and cannot be added to admission or construction wall time. Segment publication was 267.49 ms median and includes 248.90 ms indexing/sealing, 16.55 ms installation and 2.25 ms callback medians. Across segments, bigram finalization was 151.49 ms and final sorting 83.69 ms, both **inside** indexing/sealing and outside admission. Waiting for ordered contributions belongs to the existing scheduling statistics, not the exclusive admission table; completion-to-receipt and sampled extraction measurements likewise remain nonadditive.

## Invariants, tests and implications

[`FileInventory::from_restored_entries`](../../../crates/core/src/inventory.rs#L782) accepts the decoded entries in serialized order, reserves the path map and calls the existing checked `rebuild_lookup()` once. [`load_snapshot_parts`](../../../crates/core/src/storage.rs#L1619) still validates file ID sequence, nonempty and in-range collection membership, and reproduces the former `add()` behavior of deduplicating repeated memberships. `rebuild_lookup()` then checks paths, uniqueness, status/diagnostics and ordered keys and builds the final maps. No source content, `stat`, canonicalization against disk, freshness check, new permanent map or snapshot-format change is involved. Both profiled and ordinary checked load entry points share this function.

[`validate_segment`](../../../crates/core/src/lifecycle_segment_codec.rs#L586) checks contiguous slots and record classes, reconstructs logical membership from those records, compares it against serialized membership, then observes that all slots are logical. Raw and custom text indexes must first pass coverage and slot-bounds validation before their now-redundant subset traversals are skipped. Mixed logical/fallback segments keep full subset scans; fallback text always checks its fallback domain. Active/liveness masks are independent and never used as the logical proof. Empty and inactive cases, wrong-class and out-of-bounds postings, corrupt serialized membership/cardinality and checksummed invalid storage identities/paths/references are covered by focused tests. Existing update/delete/move, diagnostic, path filter, ordered range, structural, FTS, Boolean and pagination suites ran in the full workspace suite. The restored B0 snapshot and identical query digests add cross-version evidence.

Validation commands on the final integration code: `cargo test --locked --workspace --all-targets --quiet` passed; focused storage and codec tests passed; `cargo check --locked -p objekte-cli -p objekte-desktop --quiet` passed; `cargo clippy --locked --workspace --all-targets --quiet` exited 0 with existing warnings. `git diff --check` passed. `cargo fmt --all -- --check` failed on extensive inherited formatting differences across CLI, core and desktop, including files untouched by this task; broad formatting was intentionally left for a separate change. No native GUI interaction campaign was required for these shared core changes.

The ranked follow-up experiments, **not implemented in this task**, are: (1) investigate field-posting merge cost at [`SegmentAggregate::admit_profiled`](../../../crates/core/src/mutation_segment.rs#L129), 277 ms of the 395 ms admission interval (258 ms in `contents`); (2) study record-inventory slot/diagnostic/ID mapping in [`SegmentAdmission::admit_batch_profiled`](../../../crates/core/src/mutation_segment.rs#L402), 82 ms; (3) examine segment sealing at [`CollectionSegment::into_indexed_profiled`](../../../crates/core/src/mutation_segment.rs#L547), 249 ms of publication indexing, including 151 ms bigram finalization and 84 ms final sorting; (4) examine the remaining checked storage and segment deserialization costs after the two loader changes, while preserving checksums and snapshot compatibility. Each is a measured interval, not a promised wall-time saving: workers overlap admission, and seal subcomponents are nested. The dominant posting cost is recorded for user review in [the performance issue](../issues/perf/incremental_field_posting_admission_cost.md). No admission algorithm, posting representation, scheduling policy, catalogue design or snapshot format was changed here.
