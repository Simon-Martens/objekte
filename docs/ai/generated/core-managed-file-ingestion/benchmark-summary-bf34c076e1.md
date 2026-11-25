# Core-managed file ingestion benchmark results

2026-11-25, Linux x86_64 (Fedora kernel 7.3.3), rustc/cargo 1.99.1, 32 available CPUs. Repaired parent: `3bf0811943b11837bf997f51ee9f3392e0670e99`; final implementation: `bf34c076e11e5b9cc94f9f635e366d9d9de0c749`. Both worktrees were built with `cargo build --locked --release -p objekte-search-core --features detailed-profiling` and the same named examples. [Raw paired runs and binary hashes](comparison-bf34c076e1/manifest.json) contain every stdout/stderr stream. The [runner](run_comparison.py) used the same `examples/musenalm` fixture, 32 requested build workers, source options and one producer for direct ingestion. It ran parent then final in each pair. All 38 invocations succeeded. Medians below use three pairs, except the separate metadata query workload, which uses five pairs.

| Workload | Parent median | Final median | Interpretation |
| --- | ---: | ---: | --- |
| Musenalm ASCII build wall time, 148,486 files and records | 647.207 ms | 681.411 ms | 5.3% slower in this sample; both parsed 148,486 sources. |
| Musenalm ASCII peak resident bytes | 649.626 MB | 677.106 MB | Catalog scalar indexes and changed layout add memory. |
| Musenalm Unicode build wall time | 745.302 ms | 735.625 ms | 1.3% faster in this sample. |
| Musenalm Unicode peak resident bytes | 670.814 MB | 706.732 MB | Same new file-level indexing cost. |
| Inventory estimated heap bytes, either alphabet | 40.276 MB | 54.530 MB | Includes the final size/time scalar index explicitly. |
| Direct singleton publish drain, 65,536 records | 90.024 ms; 727,987 records/s | 92.378 ms; 709,437 records/s | 2.6% slower; preaccepted batches, not end-to-end producer time. |
| Direct 256-record batch publish drain, 65,536 records | 191.410 ms; 342,386 records/s | 187.007 ms; 350,447 records/s | 2.3% faster. |
| Direct 256-record overflow publish drain | 115.534 ms; 567,246 records/s | 109.733 ms; 597,231 records/s | 5.0% faster; both used the spool. |
| Direct singleton / batch / overflow peak RSS | 231.3 / 119.3 / 185.2 MiB | 225.3 / 121.9 / 110.0 MiB | Overflow RSS varies substantially with allocator and publication timing; see raw samples. |

The small lifecycle fixture gave parent/final median build 3.098/3.151 ms, add 0.212/0.222 ms, append update 0.128/0.137 ms, verified update 0.150/0.196 ms, move 0.040/0.035 ms and reconciliation 0.119/0.120 ms. The mutation differences are in the microsecond range, so they have no pass/fail threshold. All phases succeeded, including delete and compaction.

The parent routing example used 65,536 linked direct records, whereas the final example uses 64 managed files plus interleaved direct records; timings and RSS are **not comparable** after the intentional API change. In the final case, a size equality found one file and projected to 1,024 records across 68 segments; the metadata filter and its OR with a direct record matched 1,024 and 1,025 records respectively, with zero source opens and bytes read. Ten thousand key lookups had zero unrelated segment visits. The final example also checked live reverse membership for all 65,536 file records.

For a comparable metadata query, the same [public API benchmark source](metadata_query_benchmark.rs) was compiled in both worktrees. It imported 64 JSON Lines files as 64 segments (16,384 records), queried one unique file size, and checked 256 matches with zero content reads. [Five paired outputs and binary/source hashes](metadata-comparison-bf34c076e1.json) show median cold query 169.590/172.235 microseconds and median 500 warm queries 65.225/65.758 ms (parent/final). No material metadata query regression appeared on this fixture.

The final [100,000-record scale run](scale-100k-bf34c076e1.json) imported one 30,000,000-byte JSON Lines file in 375.570 ms (266,262 records/s). The catalog had one file and 100,000 live records; the last span was `[29,999,700, 29,999,999)`. One source content open read exactly 30,000,000 bytes. Peak in-flight input reservation was 4,915,200 bytes, peak RSS 78,196 KiB, 391 chunks/segments were completed, and initial build reported 391 searchable batches. The 391 batch count describes incremental initial construction; the lifecycle update tests verify all-or-nothing publication of a replacement file.

The first scalar implementation used a full `Number` value for every size key. Its [intermediate comparison](comparison-4a049f5fdd/manifest.json) showed 61.657 MB estimated inventory heap. Compacting the size key to `u64` reduced that to 54.530 MB in the final build while preserving fractional numeric comparisons. A [pre-scalar comparison](comparison-939985aa32/manifest.json) is retained for the regression audit. Remaining build time and memory differences reflect the required file-level scalar indexes and run-to-run variation; the paired data do not support a further safe shortcut that retains their lookup behavior.
