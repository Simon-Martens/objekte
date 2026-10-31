# Indexed JSON counts after snapshot planning — 2026-10-31

The same 50-query release workload was rerun after adding null and inequality postings and counting exact-index matches from snapshot bits. One record is returned per query; exact totals cover the full schema-compatible scope. Each median uses seven timed runs after one untimed warmup per filter. All 50 match counts agree with the baseline.

| Filter | Matches | Before ms | After ms | Before scans | After scans |
|---|---:|---:|---:|---:|---:|
| `exists(annotation)` | 148,478 | 167.82 | 1.33 | 0 | 0 |
| `year = null` | 0 | 121.66 | 0.55 | 138,876 | 0 |
| `year = 1800` | 2,989 | 5.05 | 0.58 | 0 | 0 |
| `year != 1800` | 135,887 | 120.61 | 0.73 | 138,876 | 0 |
| `year >= 1800` | 98,974 | 116.16 | 0.66 | 0 | 0 |
| `year < 1800` | 39,902 | 52.15 | 0.61 | 0 | 0 |
| `year >= 1800 && year < 1805` | 14,196 | 20.22 | 0.70 | 0 | 0 |
| `year = 1800 || year = 1801` | 5,692 | 7.39 | 0.65 | 0 | 0 |
| `pseudonym = true` | 11,898 | 16.79 | 0.53 | 0 | 0 |
| `pseudonym = false` | 126,978 | 143.57 | 0.92 | 0 | 0 |
| `year = 1800 && pseudonym = true` | 223 | 1.06 | 0.73 | 0 | 0 |
| `(year = 1800 || year = 1801) && pseudonym = true` | 385 | 1.19 | 0.70 | 0 | 0 |

Broad exact counts now use bitmap popcounts and select only the requested page IDs. `year = null` and `year != 1800` now use typed postings. A returned page record is still read and validated; an indexed count is a snapshot count until the index is explicitly refreshed or rebuilt. Scalar paths that could yield multiple values retain the source-scan fallback.

Index warmup was 726.5 ms before and 728.3 ms after, excluded from query medians. The raw [baseline](results/musenalm-mix-50-2026-10-31.ndjson) and [after-change](results/musenalm-mix-50-after-2026-10-31.ndjson) runs contain all 50 cases and scan/parse metrics.
