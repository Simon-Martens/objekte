# Direct indexed sort path — 2026-11-01

The repository now emits verified JSON and full-text page results directly in a matching schema sort index's order. It skips the result sort buffer and stops verification after enough matches to determine the repository's capped total and truncation flag. Queries without a matching index retain the bounded sort buffer. The index now uses the schema's root `x-language`, which previously applied only in the final sort buffer.

## 50-query benchmark

The [sorting workload](workloads/musenalm-sort-mix-50.json) searched each query's relevant Musenalm collection with a 25-record page and a 10,000-result repository cap. Both release binaries used the same source tree and warm OS cache. The baseline binary was built from commit `15206a8119`; the new binary used this change. Each runner made one untimed warm pass and three timed rounds, alternating query order. Numbers below sum the 50 per-query medians; they are not one round's wall time. This paired rerun followed an initial baseline/new pair with the same result pattern.

| Queries | Baseline | Direct indexed path | Change |
|---|---:|---:|---:|
| 13 with a matching sort index | 1,008.4 ms | 541.8 ms | 1.86× faster |
| 37 without a matching sort index | 6,018.0 ms | 5,977.4 ms | 0.7% faster |
| All 50 | 7,026.4 ms | 6,519.2 ms | 7.2% faster |

All 50 capped totals, truncation flags, and sort-index selections matched. Each runner checked that page IDs remained stable across its warm pass and timed rounds. Core tests compare indexed pages and full-text hits against direct execution across German, English, and Swedish collation, pagination, mixed filters, and result caps. A restored index is also checked after changing the root language.

| # | Sort | Hits | Baseline ms | Direct ms |
|---:|---|---:|---:|---:|
| 2 | `preferred_title ASC` | 10000+ | 105.2 | 61.3 |
| 4 | `preferred_title DESC` | 10000+ | 133.4 | 74.6 |
| 5 | `year ASC && preferred_title ASC` | 10000+ | 112.5 | 60.1 |
| 8 | `preferred_title ASC` | 10000+ | 100.2 | 59.2 |
| 9 | `preferred_title DESC` | 10000+ | 121.7 | 61.5 |
| 10 | `year ASC && preferred_title ASC` | 10000+ | 93.3 | 32.8 |
| 12 | `@id DESC` | 5516 | 49.5 | 20.1 |
| 17 | `preferred_title DESC` | 379 | 4.8 | 2.8 |
| 20 | `preferred_title ASC` | 909 | 16.9 | 9.5 |
| 24 | `preferred_title DESC` | 909 | 16.3 | 8.2 |
| 31 | `preferred_title ASC` | 0 | 33.7 | 38.3 |
| 36 | `year ASC && preferred_title ASC` | 10000+ | 113.0 | 60.8 |
| 40 | `preferred_title DESC` | 10000+ | 107.9 | 52.6 |

Raw runs: [baseline](results/musenalm-sort-index-direct-baseline-2026-11-01.ndjson) and [direct path](results/musenalm-sort-index-direct-new-2026-11-01.ndjson). Timings are sensitive to file cache and machine load; the nearly unchanged unindexed group is a useful control for this run.
