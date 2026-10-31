# Musenalm 50-query comparison: with and without schemas

Release build, 31 October 2026. Both modes searched all seven Musenalm collections with one warm repository handle, counted every match, and returned one record. The same 50 filters were run once to warm source pages and then seven timed rounds in alternating order. Values below are per-filter wall-clock medians; the operating system file cache was not controlled. The source repository was not modified.

- **With schemas:** 773.3 ms to warm indexes; 2758.4 ms summed query medians; 2,610,493 scans; 1,852,562 parses.
- **Without schemas:** 499.4 ms to warm indexes; 4008.1 ms summed query medians; 4,059,814 scans; 3,297,931 parses.
- All 50 match counts agreed. Schema mode used the configured schema postings and validation; schema-free mode discovered the same folders while ignoring default schemas and did not build declared ordered postings.
- The sum of medians describes the workload; it is not one end-to-end run. Warmup is reported separately. The schema-free run followed the schema run, so small same-workload differences may reflect OS cache drift.

| # | Filter | Matches | Schema ms | No schema ms | Schema scans | No schema scans |
|---:|---|---:|---:|---:|---:|
| 1 | `annotation ~ 'Mond'` | 45 | 32.60 | 32.26 | 8,790 | 8,790 |
| 2 | `annotation !~ 'Mond'` | 148,433 | 130.28 | 129.13 | 148,486 | 148,486 |
| 3 | `annotation ~ :ignorecase('Mond')` | 51 | 48.71 | 48.31 | 8,790 | 8,790 |
| 4 | `annotation !~ :ignorecase('Mond')` | 148,427 | 135.71 | 134.01 | 148,486 | 148,486 |
| 5 | `exists(annotation)` | 148,478 | 1.48 | 125.94 | 8 | 148,486 |
| 6 | `year = null` | 0 | 11.43 | 125.59 | 9,610 | 148,486 |
| 7 | `year = 1800` | 2,989 | 11.24 | 126.54 | 9,610 | 148,486 |
| 8 | `year != 1800` | 135,887 | 11.26 | 126.08 | 9,610 | 148,486 |
| 9 | `year >= 1800` | 98,974 | 11.08 | 126.88 | 9,610 | 148,486 |
| 10 | `year < 1800` | 39,902 | 11.06 | 126.67 | 9,610 | 148,486 |
| 11 | `year >= 1800 && year < 1805` | 14,196 | 11.27 | 128.18 | 9,610 | 148,486 |
| 12 | `year = 1800 || year = 1801` | 5,692 | 11.12 | 127.35 | 9,610 | 148,486 |
| 13 | `pseudonym = true` | 11,898 | 5.39 | 55.17 | 5,986 | 22,025 |
| 14 | `pseudonym = false` | 126,978 | 11.17 | 127.15 | 8,407 | 147,283 |
| 15 | `musenalm_type ?~ 'Gedicht'` | 59,586 | 81.17 | 80.49 | 71,550 | 71,550 |
| 16 | `year = 1800 && pseudonym = true` | 223 | 5.76 | 54.99 | 5,986 | 22,025 |
| 17 | `(year = 1800 || year = 1801) && pseudonym = true` | 385 | 7.52 | 58.28 | 5,986 | 22,025 |
| 18 | `numbering = 0.1` | 95 | 18.67 | 96.09 | 14,538 | 148,486 |
| 19 | `numbering < year` | 133,156 | 96.59 | 96.09 | 148,486 | 148,486 |
| 20 | `preferred_title ~ :ignorecase('almanach')` | 2,224 | 45.77 | 45.11 | 6,544 | 6,544 |
| 21 | `preferred_title ~ 'Mond'` | 274 | 32.46 | 31.76 | 8,790 | 8,790 |
| 22 | `preferred_title !~ 'Mond'` | 138,602 | 129.16 | 127.65 | 148,486 | 148,486 |
| 23 | `preferred_title ~ :ignorecase('Taschen%')` | 1,899 | 51.90 | 51.05 | 11,571 | 11,571 |
| 24 | `preferred_title:lower ~ 'almanach'` | 2,224 | 33.08 | 33.27 | 6,544 | 6,544 |
| 25 | `preferred_title = 'Der Mond'` | 12 | 29.14 | 28.34 | 7,718 | 7,718 |
| 26 | `musenalm_type ?!~ 'Gedicht'` | 74,931 | 130.34 | 128.99 | 148,486 | 148,486 |
| 27 | `name ~ :ignorecase('Goethe')` | 1 | 27.91 | 27.64 | 3,257 | 3,257 |
| 28 | `profession = 'Text'` | 1,520 | 67.32 | 66.33 | 33,099 | 33,099 |
| 29 | `corporate_body = true` | 753 | 53.61 | 55.55 | 20,235 | 22,025 |
| 30 | `created < updated` | 8,570 | 128.67 | 128.18 | 148,486 | 148,486 |
| 31 | `language ?= 'ger'` | 4,922 | 61.89 | 61.96 | 31,050 | 31,050 |
| 32 | `language ?!= 'ger'` | 3 | 124.07 | 122.46 | 148,486 | 148,486 |
| 33 | `language:each = 'ger'` | 4,922 | 61.99 | 62.01 | 31,050 | 31,050 |
| 34 | `contents:length >= 50` | 1,061 | 109.51 | 107.77 | 148,486 | 148,486 |
| 35 | `contents.year ?>= 1800` | 1,768 | 111.30 | 108.06 | 148,486 | 148,486 |
| 36 | `exists(contents[0].source_id)` | 2,394 | 109.99 | 107.66 | 148,486 | 148,486 |
| 37 | `places ?= 'd7kbujq7399g22j'` | 888 | 16.60 | 15.80 | 1,013 | 1,013 |
| 38 | `agents.type ?= 'Verlag'` | 3,653 | 47.65 | 46.07 | 8,210 | 8,210 |
| 39 | `contributors.agents ?= '5e64t7ca8x2wyim'` | 121 | 7.05 | 6.44 | 284 | 284 |
| 40 | `musenalm_type ?= 'Gedicht/Lied'` | 59,586 | 79.63 | 77.72 | 65,321 | 65,321 |
| 41 | `contributors.type ?= 'Autor:in'` | 70,390 | 86.75 | 85.34 | 73,738 | 73,738 |
| 42 | `contributors:length > 0` | 70,428 | 97.09 | 95.99 | 148,486 | 148,486 |
| 43 | `entries:length >= 3` | 341 | 80.89 | 78.97 | 148,486 | 148,486 |
| 44 | `entries ?= '1o25nw5kc1t9ko4'` | 1 | 4.64 | 4.84 | 148 | 148 |
| 45 | `owner = 'Theodor Springmann Stiftung' && basis = true` | 4,176 | 50.60 | 50.39 | 8,119 | 8,122 |
| 46 | `media ?= 'Reproduktion'` | 46 | 10.51 | 10.54 | 1,061 | 1,061 |
| 47 | `basis = false` | 3 | 125.11 | 126.52 | 143,103 | 147,283 |
| 48 | `fictional = true` | 7 | 53.66 | 55.51 | 20,228 | 22,025 |
| 49 | `key = 'kontakt'` | 1 | 10.32 | 10.46 | 1,781 | 1,781 |
| 50 | `data.description ~ 'Musenalm'` | 7 | 126.22 | 124.55 | 148,486 | 148,486 |

## Main observation

Schema postings made 13 filters at least twice as fast, especially `exists(annotation)`, year comparisons, `pseudonym` comparisons, and numeric equality. Most substring, array, nested-path, negation, and field-to-field filters scanned the same files in both modes and had similar timings. The schema-free index warmed faster because it built no schema-declared ordered postings.

## Raw results and reproduction

- [With schemas](results/musenalm-mix-50-with-schema-2026-10-31.ndjson)
- [Without schemas](results/musenalm-mix-50-without-schema-2026-10-31.ndjson)

```sh
cargo build --release -p objekte-bench --bin json-query-mix
target/release/json-query-mix --repo examples/musenalm --runs 7 > crates/bench/results/musenalm-mix-50-with-schema-2026-10-31.ndjson
target/release/json-query-mix --repo examples/musenalm --runs 7 --no-schema > crates/bench/results/musenalm-mix-50-without-schema-2026-10-31.ndjson
```
