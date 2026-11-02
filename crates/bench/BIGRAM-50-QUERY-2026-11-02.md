# 50-query search benchmark: ASCII versus UTF-8 byte bigrams — 2 November 2026

The existing `musenalm-mix-50.json` workload ran against all seven configured Musenalm collections.
Each query counted matches up to the repository’s 10,000-result cap and returned one record.
After warming every index and running an untimed query pass, each process executed seven timed
rounds with alternating forward and reverse query order. Three separate release processes ran per
encoding, alternating process order. Each table time is the median of the three process medians.
Index warming and process startup are excluded. The OS file cache was warm and uncontrolled.
The machine was an Intel Core i9-13900K.

All 50 queries returned the same capped count and first record ID in all six processes.
The summed source-read count was 1,543,542 in each process for both encodings. Most workload
literals are ASCII, and many filters use typed indexes, so this workload offers little opportunity
for UTF-8 byte postings to narrow candidates further.

| Measure | ASCII | UTF-8 bytes |
|---|---:|---:|
| Sum of 50 per-query medians | 1452.96 ms | 1467.27 ms |
| Median query time | 10.24 ms | 9.90 ms |
| Faster cases | 29 | 21 |

The UTF-8 sum is 1.0% higher. The per-case differences are small and change direction across
queries, so this run supports near-equal search latency for this mostly ASCII workload.

| # | Filter | Matches (capped) | ASCII ms | UTF-8 ms | UTF-8 change |
|---:|---|---:|---:|---:|---:|
| 1 | `annotation ~ 'Mond'` | 45 | 11.47 | 10.22 | -10.9% |
| 2 | `annotation !~ 'Mond'` | 10,000 | 132.78 | 129.59 | -2.4% |
| 3 | `annotation ~ :ignorecase('Mond')` | 51 | 19.11 | 20.23 | +5.8% |
| 4 | `annotation !~ :ignorecase('Mond')` | 10,000 | 125.56 | 129.01 | +2.7% |
| 5 | `exists(annotation)` | 10,000 | 7.49 | 7.55 | +0.8% |
| 6 | `year = null` | 0 | 2.98 | 2.99 | +0.4% |
| 7 | `year = 1800` | 2,989 | 3.63 | 3.59 | -1.1% |
| 8 | `year != 1800` | 10,000 | 3.17 | 3.16 | -0.1% |
| 9 | `year >= 1800` | 10,000 | 3.07 | 3.07 | 0.0% |
| 10 | `year < 1800` | 10,000 | 3.03 | 3.13 | +3.5% |
| 11 | `year >= 1800 && year < 1805` | 10,000 | 5.32 | 5.44 | +2.3% |
| 12 | `year = 1800 \|\| year = 1801` | 5,692 | 4.02 | 4.07 | +1.2% |
| 13 | `pseudonym = true` | 10,000 | 4.74 | 4.81 | +1.3% |
| 14 | `pseudonym = false` | 10,000 | 4.71 | 4.69 | -0.4% |
| 15 | `musenalm_type ?~ 'Gedicht'` | 10,000 | 48.43 | 48.77 | +0.7% |
| 16 | `year = 1800 && pseudonym = true` | 223 | 2.96 | 3.00 | +1.4% |
| 17 | `(year = 1800 \|\| year = 1801) && pseudonym = true` | 385 | 3.45 | 3.44 | -0.2% |
| 18 | `numbering = 0.1` | 95 | 7.34 | 7.33 | -0.1% |
| 19 | `numbering < year` | 10,000 | 89.70 | 90.40 | +0.8% |
| 20 | `preferred_title ~ :ignorecase('almanach')` | 2,224 | 10.96 | 11.43 | +4.3% |
| 21 | `preferred_title ~ 'Mond'` | 274 | 7.01 | 7.14 | +1.8% |
| 22 | `preferred_title !~ 'Mond'` | 10,000 | 106.71 | 105.79 | -0.9% |
| 23 | `preferred_title ~ :ignorecase('Taschen%')` | 1,899 | 15.47 | 15.98 | +3.3% |
| 24 | `preferred_title:lower ~ 'almanach'` | 2,224 | 13.39 | 14.04 | +4.8% |
| 25 | `preferred_title = 'Der Mond'` | 12 | 12.64 | 13.33 | +5.5% |
| 26 | `musenalm_type ?!~ 'Gedicht'` | 10,000 | 96.74 | 97.96 | +1.3% |
| 27 | `name ~ :ignorecase('Goethe')` | 1 | 7.58 | 7.48 | -1.3% |
| 28 | `profession = 'Text'` | 1,520 | 2.88 | 2.88 | +0.0% |
| 29 | `corporate_body = true` | 753 | 2.75 | 2.74 | -0.2% |
| 30 | `created < updated` | 8,570 | 104.55 | 111.80 | +6.9% |
| 31 | `language ?= 'ger'` | 4,922 | 27.06 | 26.29 | -2.9% |
| 32 | `language ?!= 'ger'` | 3 | 92.93 | 95.07 | +2.3% |
| 33 | `language:each = 'ger'` | 4,922 | 26.32 | 26.54 | +0.8% |
| 34 | `items:length >= 1` | 3,913 | 11.19 | 9.78 | -12.6% |
| 35 | `entry = '00fx8ln651na0e3'` | 81 | 8.14 | 8.16 | +0.3% |
| 36 | `exists(entry)` | 10,000 | 4.17 | 4.03 | -3.2% |
| 37 | `places ?= 'd7kbujq7399g22j'` | 888 | 13.46 | 13.06 | -3.0% |
| 38 | `agents.type ?= 'Verlag'` | 3,653 | 12.05 | 11.88 | -1.4% |
| 39 | `contributors.agents ?= '5e64t7ca8x2wyim'` | 121 | 10.12 | 10.26 | +1.4% |
| 40 | `musenalm_type ?= 'Gedicht/Lied'` | 10,000 | 56.72 | 56.42 | -0.5% |
| 41 | `contributors.type ?= 'Autor:in'` | 10,000 | 59.09 | 59.37 | +0.5% |
| 42 | `contributors:length > 0` | 10,000 | 91.56 | 91.86 | +0.3% |
| 43 | `entries:length >= 3` | 341 | 6.99 | 7.07 | +1.2% |
| 44 | `entries ?= '1o25nw5kc1t9ko4'` | 1 | 9.77 | 9.78 | +0.2% |
| 45 | `owner = 'Theodor Springmann Stiftung' && basis = true` | 4,176 | 31.80 | 31.75 | -0.1% |
| 46 | `media ?= 'Reproduktion'` | 46 | 10.37 | 10.01 | -3.4% |
| 47 | `basis = false` | 3 | 4.24 | 4.22 | -0.6% |
| 48 | `fictional = true` | 7 | 2.66 | 2.63 | -1.0% |
| 49 | `key = 'kontakt'` | 1 | 6.06 | 6.07 | +0.2% |
| 50 | `data.description ~ 'Musenalm'` | 7 | 104.60 | 107.92 | +3.2% |

## Raw runs

- ascii, process 1: [NDJSON](results/musenalm-mix-50-ascii-2026-11-02.ndjson), sum 1444.01 ms; index warm 1680.8 ms; 7 snapshots restored.
- ascii, process 2: [NDJSON](results/musenalm-mix-50-ascii-2026-11-02-repeat2.ndjson), sum 1484.36 ms; index warm 2140.9 ms; 0 snapshots restored.
- ascii, process 3: [NDJSON](results/musenalm-mix-50-ascii-2026-11-02-repeat3.ndjson), sum 1434.78 ms; index warm 663.0 ms; 7 snapshots restored.
- utf8_bytes, process 1: [NDJSON](results/musenalm-mix-50-utf8-bytes-2026-11-02.ndjson), sum 1481.14 ms; index warm 2476.9 ms; 0 snapshots restored.
- utf8_bytes, process 2: [NDJSON](results/musenalm-mix-50-utf8-bytes-2026-11-02-repeat2.ndjson), sum 1441.18 ms; index warm 674.4 ms; 7 snapshots restored.
- utf8_bytes, process 3: [NDJSON](results/musenalm-mix-50-utf8-bytes-2026-11-02-repeat3.ndjson), sum 1484.78 ms; index warm 2495.0 ms; 0 snapshots restored.

Run `target/release/json-query-mix --repo examples/musenalm --runs 7 --bigram-encoding ascii`
or replace `ascii` with `utf8_bytes`. The [bench README](README.md) has build instructions.
