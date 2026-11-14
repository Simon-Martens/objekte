# Direct snapshot access implementation and measurements

Task: `direct-snapshot-access`. Integration branch: `integration/direct-snapshot-access`. Base: `dd3f8feb19df68dd5a5a3ada0d8c99ca91336524`. Ordinary release performance implementation: `8747efc47e`; final implementation includes feature-gated diagnostic instrumentation at `175c94bc15`. The candidate and baseline executable SHA-256 values are recorded with each [raw sample set](../../../bench/results/direct-snapshot-access/final-20261114/README.md). The original checkout remained on `master`.

## Delivered access paths

| Structure | Persisted reader and remaining work |
| --- | --- |
| Gram presence | Section 11 holds checked filter records, populated while writing dictionary keys and borrowed at open. No filter construction traversal at open/build. Untrusted dictionary validation checks coverage in its existing pass. |
| Segment file/record slots | Fixed file and record rows, borrowed IDs, persisted membership and logical/fallback Roaring domains. No installed file-ID map, record-ID map, or per-file record lists. Record ID search uses persisted identity order. |
| Catalog | Version 4 slot, historical, native-path and query-path tables plus active bitmap. Query and mutation use direct base readers plus bounded mutable overlay. No all-file `FileEntry` installation, path sort, hash lookup, or ordered path-tree construction at open. |
| Old formats | Persisted obsolete generations are checked, retired into aged GC, and rebuilt under the existing typed recovery rules. Unchanged current segments remain reusable across flushes. |

Current format descriptions are in `docs/development/direct_snapshot_access_format.md`, `direct_snapshot_segment_format.md`, and `direct_snapshot_catalog_format.md`; the [validation inventory](direct_snapshot_access_validation_inventory.md) lists each retained scan and its purpose. Whole-section SHA checks still touch the checked section bytes. A full catalog semantic validation and one linear record/membership validation pass remain before publication. Cross-segment live-ID validation streams IDs through a heap. The segment validation visits 148,486 record slots in this fixture. No source files are read on measured managed opens. The diagnostic runner reports zero lookup reconstruction and startup path sorting by the structural access paths; it does not separately count every dictionary key visited or every transient path/string decode during validation.

## Comparable measurements

Baseline and candidate used the same Musenalm fixture, release settings, 32 workers, 16 MiB prepared-cache budget, zero verified-cache bytes, and matching result digests. The full campaign covered 220 cases in each ASCII and Unicode profile, in memory, fully resident and on-demand modes, 30 repetitions per case. Warm opens and fresh builds used seven paired trials. All ordinary managed-open trials were source-free. See raw JSON for exact commands and sample values.

| Warm managed open, seven-trial median | Baseline | Candidate | Change |
| --- | ---: | ---: | ---: |
| On-demand ASCII | 391.823 ms | 244.774 ms | 37.5% faster |
| On-demand Unicode | 399.865 ms | 234.445 ms | 41.4% faster |
| Resident ASCII | 449.435 ms | 300.617 ms | 33.1% faster |
| Resident Unicode | 482.162 ms | 300.362 ms | 37.7% faster |

First Q167 after open improved from 8.506 to 7.824 ms (on-demand ASCII), 10.216 to 8.655 ms (on-demand Unicode), 8.041 to 7.948 ms (resident ASCII), and 9.048 to 8.288 ms (resident Unicode). Thus the open improvement was not shifted into that first query. After `POSIX_FADV_DONTNEED`, three paired runs with `fincore` reporting zero cached bytes gave ASCII opens of 423.317 to 272.009 ms and Unicode opens of 442.454 to 259.240 ms. This is a page-eviction experiment, not a cold boot; first-query variance in these three trials was larger.

The full query campaign had identical result digests. Candidate/baseline geometric mean ratios of per-case medians were 1.006 in memory, 1.107 resident, and 1.092 on-demand; p95 ratios were 1.006, 1.095, and 1.073. Summing all per-case medians gave ratios 1.014, 0.948, and 0.949 respectively, because broad scans improved while numerous short cases regressed. Examples in the ASCII on-demand profile: broad modified-time Q119 improved 4.228 to 2.610 ms; sorted entries Q151 improved 0.601 to 0.361 ms; backlink Q170 improved 0.049 to 0.026 ms; narrow size Q109 regressed 0.067 to 0.122 ms; `@today` Q184 regressed 0.192 to 0.256 ms. These small-query regressions remain a performance tradeoff and are visible in every per-case row in the archive. The latest user instruction prioritized finishing over further millisecond-level tuning.

| Other seven-trial medians | Baseline | Candidate | Change |
| --- | ---: | ---: | ---: |
| Fresh incremental build | 1,166.669 ms | 1,191.927 ms | 2.2% slower (median paired ratio 1.014) |
| First build batch | 366.361 ms | 374.497 ms | 2.2% slower (median paired ratio 1.040) |
| Initial ASCII flush | 233.826 ms | 315.861 ms | 82.036 ms slower |
| Unchanged flush | 1.131 µs | 0.995 µs | effectively unchanged |
| Stored ASCII snapshot | 131,082,617 B | 141,616,861 B | 8.0% larger |

The initial flush writes and checks about 10.5 MB of additional persisted lookup data; its 82 ms cost is deferred publication work, not a free startup saving. On a separate 16,384-file JSONL fixture, three paired first-update-and-flush medians after restore were 12.422 to 13.279 ms; the initial flush there was 35.432 to 40.784 ms. There is no delayed full lookup reconstruction on the first update. The ASCII on-demand process RSS at open fell from 272.9 to 189.9 MiB; Unicode from 543.9 to 335.9 MiB. Managed snapshot bytes rose about 8% in both profiles. Counting-allocation opens show gross allocated bytes falling from 449.9 to 248.8 MB in ASCII on-demand and from 555.1 to 361.0 MB in ASCII resident, while allocation calls rose from 3.30 to 3.50 million. The extra small calls likely arise in direct validation/overlay setup; this attribution is an inference, not a per-site allocation trace.

Feature-gated candidate phase tracing (three runs) found an ASCII on-demand median of 247.96 ms overall: 28.58 ms catalog read, 19.20 ms segment section checks, 19.36 ms catalog semantic validation, 152.87 ms segment installation, and 19.24 ms cross-segment live-ID validation. Within segment installation, record validation took 62.29 ms, dictionary installation 13.86 ms, sort/relation decoding 47.57 ms, and final checks 29.25 ms. These overlapping nested categories explain remaining startup cost; they must not be added together with their parent. Metadata readiness occurred at 72 ms and full readiness at 247 ms in this instrumented mode. Whole-section checks, validation, and sort/relation decoding are the dominant remaining work, with the full scans retained to protect external-input correctness.

## Validation and limits

`CARGO_TARGET_DIR=/home/simon/source/objekte-integration-hybrid-index-storage/target/direct-snapshot-access/integration-build cargo test --locked --workspace` passed on the combined branch after the diagnostic commit. The detailed-profiling release example built and ran all phase samples. The earlier ordinary release benchmark examples built without warnings. Validation covers corrupt sections and table references, current/obsolete generation behavior, query equivalence, resident/on-demand readers, and first update after restore through the workspace suite and benchmark comparisons. No query digest mismatch or source access occurred in measured persisted opens.

The query geometric means and initial flush are regressions; the task does not claim an across-the-board speedup. Their raw samples and tradeoff are recorded for review. The current interface still owns bounded sort/relation leaf structures and decoded Roaring domains; further removal would require a separate representation change. The full validation passes and section SHA checks remain intentional. Exact per-purpose dictionary visitation and transient string-decode counters were not collected, so the validation inventory records those traversals structurally instead.
