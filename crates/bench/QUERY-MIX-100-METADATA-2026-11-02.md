# 100-query Musenalm filter benchmark: 50 metadata cases — 2 November 2026

The new [100-case workload](workloads/musenalm-mix-100.json) preserves all 50 cases from the [original workload](workloads/musenalm-mix-50.json) and adds 50 metadata filters. Each query searches all seven configured Musenalm collections, counts matches up to the repository cap of 10,000, and returns one record. One untimed pass precedes three timed release rounds, with alternating query order. Times below are per-query medians; their sums are not a single round’s wall time.

| Group | ASCII sum of medians | UTF-8 byte sum of medians |
| --- | ---: | ---: |
| Original JSON filters, 50 | 1853.7 ms | 1774.1 ms |
| New metadata filters, 50 | 5991.3 ms | 6026.8 ms |
| All 100 | 7845.0 ms | 7800.8 ms |

All 100 cases produced identical match totals, first IDs, candidate scans, and parses under both index encodings. The ASCII run restored seven snapshots; the UTF-8 byte run rebuilt them, so index warmup times are not directly comparable. The timed queries use warm indexes.

Metadata filters currently have no dedicated posting index. Most of these cases therefore scan all 148,486 file candidates, even when the final match count is small. Cases 97–99 combine metadata with JSON predicates and can use ordinary JSON postings or bigram candidates. `@status` also validates parseable candidates and is more expensive. File modification and creation times depend on the local checkout and filesystem; match totals may differ elsewhere. A one-round schema-free run also completed all 100 cases and returned `no_schema` for case 90.

| # | Metadata filter | Matches | ASCII median (ms) | UTF-8 median (ms) | Candidate scans |
| ---: | --- | ---: | ---: | ---: | ---: |
| 51 | `@id ~ '0%'` | 4,215 | 122.43 | 120.41 | 148,486 |
| 52 | `@id ~ 'z%'` | 4,163 | 119.08 | 119.16 | 148,486 |
| 53 | `@id = '00fx8ln651na0e3'` | 1 | 118.34 | 120.31 | 148,486 |
| 54 | `@filename ~ '0%.json'` | 4,215 | 121.39 | 119.80 | 148,486 |
| 55 | `@filename ~ '%a.json'` | 4,130 | 120.22 | 121.82 | 148,486 |
| 56 | `@filename = '00fx8ln651na0e3.json'` | 1 | 117.28 | 118.89 | 148,486 |
| 57 | `@path ~ 'data/contents/%'` | 10,000 | 122.15 | 122.70 | 148,486 |
| 58 | `@path ~ 'data/agents/%'` | 4,010 | 119.81 | 121.93 | 148,486 |
| 59 | `@path ~ 'data/pages/%'` | 8 | 121.78 | 120.93 | 148,486 |
| 60 | `@path ~ '%/00fx8ln651na0e3.json'` | 1 | 122.32 | 120.38 | 148,486 |
| 61 | `@size < 500B` | 5,323 | 117.90 | 116.15 | 148,486 |
| 62 | `@size >= 1KB` | 10,000 | 119.14 | 118.79 | 148,486 |
| 63 | `@size >= 1KiB` | 10,000 | 120.13 | 116.63 | 148,486 |
| 64 | `@size > 1.5KB` | 10,000 | 115.84 | 116.72 | 148,486 |
| 65 | `@size < 2KiB` | 10,000 | 116.73 | 117.36 | 148,486 |
| 66 | `@size >= 1MB` | 0 | 117.02 | 116.45 | 148,486 |
| 67 | `@size >= 1KB && @size < 2KB` | 10,000 | 119.35 | 118.10 | 148,486 |
| 68 | `@size >= 4KiB && @size < 8KiB` | 868 | 117.51 | 119.29 | 148,486 |
| 69 | `@size = 1KB` | 134 | 117.16 | 117.05 | 148,486 |
| 70 | `@size != 1KB` | 10,000 | 116.59 | 119.06 | 148,486 |
| 71 | `@modified = 2026-10` | 10,000 | 117.12 | 117.29 | 148,486 |
| 72 | `@modified = 2026-10-26` | 9,610 | 118.73 | 118.05 | 148,486 |
| 73 | `@modified = 2026-10-31` | 10,000 | 115.06 | 116.07 | 148,486 |
| 74 | `@modified < 2026-10-31` | 9,610 | 115.44 | 119.94 | 148,486 |
| 75 | `@modified <= 2026-10-26` | 9,610 | 117.89 | 118.41 | 148,486 |
| 76 | `@modified > 2026-10-26` | 10,000 | 115.99 | 116.98 | 148,486 |
| 77 | `@modified >= 2026-10-31` | 10,000 | 118.09 | 118.15 | 148,486 |
| 78 | `@modified != 2026-10-31` | 9,610 | 117.21 | 115.93 | 148,486 |
| 79 | `2026-10-26 < @modified` | 10,000 | 115.78 | 117.53 | 148,486 |
| 80 | `@modified >= 2026-10-26T14:50Z` | 10,000 | 115.04 | 119.29 | 148,486 |
| 81 | `@created = null` | 0 | 118.67 | 116.23 | 148,486 |
| 82 | `@created != null` | 10,000 | 117.86 | 117.32 | 148,486 |
| 83 | `@created < 2026-10-31` | 10,000 | 117.61 | 116.90 | 148,486 |
| 84 | `@created >= 2026-10` | 10,000 | 116.66 | 117.85 | 148,486 |
| 85 | `@created < 9999-12` | 10,000 | 115.91 | 119.60 | 148,486 |
| 86 | `@modified < @created` | 0 | 117.09 | 117.57 | 148,486 |
| 87 | `@status = 'valid'` | 10,000 | 166.64 | 167.64 | 148,486 |
| 88 | `@status = 'invalid'` | 0 | 164.96 | 165.39 | 148,486 |
| 89 | `@status = 'incorrect_json'` | 0 | 167.49 | 165.47 | 148,486 |
| 90 | `@status = 'no_schema'` | 0 | 167.76 | 166.47 | 148,486 |
| 91 | `@status != 'invalid'` | 10,000 | 165.90 | 165.76 | 148,486 |
| 92 | `@size > 1KB && @path ~ 'data/contents/%'` | 10,000 | 120.77 | 119.95 | 148,486 |
| 93 | `@path ~ 'data/agents/%' && @filename ~ '0%'` | 110 | 122.01 | 119.45 | 148,486 |
| 94 | `@modified = 2026-10-31 && @path ~ 'data/entries/%'` | 4,928 | 120.93 | 119.91 | 148,486 |
| 95 | `@created != null && @size > 2KB` | 6,599 | 118.05 | 117.53 | 148,486 |
| 96 | `@status = 'valid' && @path ~ 'data/pages/%'` | 8 | 171.98 | 172.28 | 148,486 |
| 97 | `year = 1800 && @size > 1KB` | 1,820 | 7.13 | 7.76 | 2,989 |
| 98 | `annotation ~ 'Mond' && @path ~ 'data/contents/%'` | 45 | 9.40 | 10.23 | 3,734 |
| 99 | `musenalm_type ?= 'Gedicht/Lied' && @modified >= 2026-10` | 10,000 | 67.00 | 96.20 | 59,586 |
| 100 | `@status = 'invalid' \|\| @size > 16KiB` | 280 | 168.99 | 167.68 | 148,486 |

Raw output: [ASCII](results/musenalm-mix-100-ascii-2026-11-02.ndjson) and [UTF-8 bytes](results/musenalm-mix-100-utf8-bytes-2026-11-02.ndjson).

```sh
cargo build --release -p objekte-bench --bin json-query-mix
target/release/json-query-mix --repo examples/musenalm --runs 3 > ascii.ndjson
target/release/json-query-mix --repo examples/musenalm --runs 3 --bigram-encoding utf8_bytes > utf8.ndjson
```
