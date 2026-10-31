# Full Musenalm benchmark — 31 October 2026

Commit `0a9b65974c`; release build with rustc 1.99.1 on Linux x86-64, Intel Core i9-13900K (32 logical CPUs). The checked-in Musenalm fixture includes the `contents.entry` reference and no embedded `entries.contents` lists.

## Method

- JSON mix: 50 unscoped filters over all seven collections; all matches counted and one record returned; seven warm timed rounds in each schema mode after index warmup and a warmup request per case.
- Bigram versus grep: 16 filters over `contents`; complete result parity checked first; three alternating timed rounds per backend, counting all matches and returning up to 50 records.
- Full text: 30 cases over `contents`; three fresh release processes, each with one warmup pass and three warm timed rounds; all matches counted and one record returned. A temporary schema overlay declares Markdown, HTML, XML, and plain text fields. The base and FTS indexes are built together.
- Times are wall-clock medians in milliseconds. The OS file cache was not controlled. The sum of per-case medians is a workload summary, not one end-to-end elapsed time.

## Summary

| Benchmark | Index build / warmup | Query result |
| --- | ---: | ---: |
| 50 JSON filters, schema | 750.27 ms | 2,267.19 ms sum of case medians; 24.44 ms across-case median |
| 50 JSON filters, no schema | 432.77 ms | 3,128.39 ms sum of case medians; 63.05 ms across-case median |
| 16-case bigram versus grep | Excluded | 4.77 ms versus 415.21 ms across-case medians; bigram faster in 16/16 cases |
| 30 full-text cases | 874.94 ms median | 1,021.84 ms sum of cross-process case medians |

JSON match counts agreed in all 50 cases across schema modes. Full-text match counts agreed across all three processes. The grep driver checked complete result parity with bigram before timing.

The full-text index held 17,913,583 bytes of base postings and 35,806,118 bytes of FTS postings (51.23 MiB combined). Ready RSS was 220.4–269.1 MiB; peak RSS during build was 453.0–458.7 MiB. Ready RSS varied across fresh processes even though posting bytes were identical.

### Finding

The `preferred_title:fts` filters in cases 11–15 selected no FTS candidates and scanned all 133,948 `contents` files (about 75–81 ms each). The schema overlay declares this field as `plain`; the current filter planner creates FTS candidate plans only for non-plain rendered fields. This is a measurable planning gap for plain `:fts` filters. Standalone full-text word search on the same field still used the index (case 23: about 1.2 ms). The older published FTS results are from the pre-migration fixture and are not directly comparable.

## All 50 JSON filters

| # | Kind | Filter | Matches | Schema ms | No schema ms | Schema scans / parses | No schema scans / parses |
| ---: | --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | substring | `annotation ~ 'Mond'` | 45 | 9.65 | 10.22 | 3,739 / 1,279 | 3,739 / 1,279 |
| 2 | negated substring | `annotation !~ 'Mond'` | 148,433 | 121.86 | 119.11 | 148,486 / 148,479 | 148,486 / 148,479 |
| 3 | case folded substring | `annotation ~ :ignorecase('Mond')` | 51 | 19.16 | 19.35 | 3,739 / 3,735 | 3,739 / 3,735 |
| 4 | negated case folded | `annotation !~ :ignorecase('Mond')` | 148,427 | 127.02 | 113.85 | 148,486 / 148,479 | 148,486 / 148,479 |
| 5 | field presence | `exists(annotation)` | 148,478 | 9.23 | 102.38 | 8 / 1 | 148,486 / 148,479 |
| 6 | null equality | `year = null` | 0 | 19.08 | 98.10 | 9,610 / 2,546 | 148,486 / 141,422 |
| 7 | numeric equality | `year = 1800` | 2,989 | 25.02 | 96.81 | 9,610 / 2,547 | 148,486 / 141,423 |
| 8 | numeric inequality | `year != 1800` | 135,887 | 25.15 | 97.05 | 9,610 / 2,547 | 148,486 / 141,423 |
| 9 | numeric lower bound | `year >= 1800` | 98,974 | 26.33 | 96.45 | 9,610 / 2,547 | 148,486 / 141,423 |
| 10 | numeric upper bound | `year < 1800` | 39,902 | 25.16 | 98.73 | 9,610 / 2,547 | 148,486 / 141,423 |
| 11 | numeric range | `year >= 1800 && year < 1805` | 14,196 | 23.79 | 99.16 | 9,610 / 2,547 | 148,486 / 141,423 |
| 12 | numeric OR | `year = 1800 \|\| year = 1801` | 5,692 | 24.43 | 98.94 | 9,610 / 2,547 | 148,486 / 141,423 |
| 13 | boolean true | `pseudonym = true` | 11,898 | 11.82 | 24.26 | 5,986 / 5,051 | 22,049 / 20,660 |
| 14 | boolean false | `pseudonym = false` | 126,978 | 21.86 | 103.65 | 8,407 / 8,394 | 147,283 / 146,024 |
| 15 | array any substring | `musenalm_type ?~ 'Gedicht'` | 59,586 | 76.72 | 54.89 | 69,636 / 66,467 | 69,636 / 66,467 |
| 16 | number AND boolean | `year = 1800 && pseudonym = true` | 223 | 11.33 | 31.77 | 5,986 / 5,051 | 22,049 / 20,660 |
| 17 | parentheses AND OR | `(year = 1800 \|\| year = 1801) && pseudonym = true` | 385 | 17.32 | 33.92 | 5,986 / 1,008 | 22,049 / 17,071 |
| 18 | fractional number | `numbering = 0.1` | 95 | 22.15 | 106.84 | 14,538 / 1 | 148,486 / 133,949 |
| 19 | field comparison | `numbering < year` | 133,156 | 116.85 | 104.42 | 148,486 / 133,949 | 148,486 / 133,949 |
| 20 | folded title substring | `preferred_title ~ :ignorecase('almanach')` | 2,224 | 17.20 | 17.45 | 4,934 / 4,237 | 4,934 / 4,237 |
| 21 | title substring | `preferred_title ~ 'Mond'` | 274 | 8.92 | 10.08 | 3,739 / 1,279 | 3,739 / 1,279 |
| 22 | negated title substring | `preferred_title !~ 'Mond'` | 138,602 | 124.17 | 125.14 | 148,486 / 141,561 | 148,486 / 141,561 |
| 23 | prefix wildcard | `preferred_title ~ :ignorecase('Taschen%')` | 1,899 | 24.46 | 24.26 | 8,704 / 7,951 | 8,704 / 7,951 |
| 24 | lowercase modifier | `preferred_title:lower ~ 'almanach'` | 2,224 | 17.76 | 17.50 | 4,934 / 4,398 | 4,934 / 4,398 |
| 25 | title equality | `preferred_title = 'Der Mond'` | 12 | 18.89 | 19.80 | 3,316 / 548 | 3,316 / 548 |
| 26 | negated array substring | `musenalm_type ?!~ 'Gedicht'` | 74,931 | 134.59 | 128.11 | 148,486 / 144,380 | 148,486 / 144,380 |
| 27 | agent name substring | `name ~ :ignorecase('Goethe')` | 1 | 10.15 | 9.50 | 1,514 / 765 | 1,514 / 765 |
| 28 | string equality | `profession = 'Text'` | 1,520 | 50.52 | 36.72 | 33,107 / 32,855 | 33,107 / 32,855 |
| 29 | agent boolean | `corporate_body = true` | 753 | 24.46 | 29.39 | 20,259 / 19,794 | 22,049 / 20,660 |
| 30 | date field comparison | `created < updated` | 8,570 | 107.01 | 110.03 | 148,486 / 148,487 | 148,486 / 148,487 |
| 31 | array any equality | `language ?= 'ger'` | 4,922 | 31.20 | 30.23 | 31,627 / 22,313 | 31,627 / 22,313 |
| 32 | array any inequality | `language ?!= 'ger'` | 3 | 102.17 | 103.14 | 148,486 / 139,202 | 148,486 / 139,202 |
| 33 | array every equality | `language:each = 'ger'` | 4,922 | 34.99 | 32.44 | 31,627 / 22,313 | 31,627 / 22,313 |
| 34 | entry item array length | `items:length >= 1` | 3,913 | 88.06 | 84.44 | 148,486 / 4,932 | 148,486 / 4,932 |
| 35 | content entry reference | `entry = '00fx8ln651na0e3'` | 81 | 8.95 | 9.86 | 9 / 2 | 90 / 83 |
| 36 | indexed entry presence | `exists(entry)` | 133,948 | 16.27 | 112.67 | 14,538 / 71 | 148,486 / 134,019 |
| 37 | relation ID in array | `places ?= 'd7kbujq7399g22j'` | 888 | 15.98 | 14.73 | 889 / 889 | 889 / 889 |
| 38 | nested array string | `agents.type ?= 'Verlag'` | 3,653 | 20.22 | 17.36 | 8,069 / 5,553 | 8,069 / 5,553 |
| 39 | nested relation ID | `contributors.agents ?= '5e64t7ca8x2wyim'` | 121 | 11.32 | 11.26 | 123 / 122 | 123 / 122 |
| 40 | enum array equality | `musenalm_type ?= 'Gedicht/Lied'` | 59,586 | 86.57 | 71.22 | 63,328 / 59,915 | 63,328 / 59,915 |
| 41 | nested enum equality | `contributors.type ?= 'Autor:in'` | 70,390 | 70.94 | 74.76 | 71,914 / 70,468 | 71,914 / 70,468 |
| 42 | contributors length | `contributors:length > 0` | 70,428 | 105.86 | 112.78 | 148,486 / 133,949 | 148,486 / 133,949 |
| 43 | series array length | `entries:length >= 3` | 341 | 85.05 | 85.18 | 148,486 / 1,205 | 148,486 / 1,205 |
| 44 | series ID membership | `entries ?= '1o25nw5kc1t9ko4'` | 1 | 9.97 | 9.92 | 1 / 2 | 1 / 2 |
| 45 | item string AND boolean | `owner = 'Theodor Springmann Stiftung' && basis = true` | 4,176 | 51.49 | 50.88 | 8,112 / 8,088 | 8,115 / 8,089 |
| 46 | item array equality | `media ?= 'Reproduktion'` | 46 | 12.16 | 12.31 | 120 / 116 | 120 / 116 |
| 47 | item boolean false | `basis = false` | 3 | 106.27 | 114.67 | 143,103 / 141,844 | 147,283 / 146,024 |
| 48 | place boolean | `fictional = true` | 7 | 22.60 | 32.09 | 20,252 / 19,787 | 22,049 / 20,660 |
| 49 | page key equality | `key = 'kontakt'` | 1 | 7.54 | 7.60 | 286 / 5 | 286 / 5 |
| 50 | open schema nested text | `data.description ~ 'Musenalm'` | 7 | 107.58 | 103.01 | 148,486 / 148,487 | 148,486 / 148,487 |

## All 16 bigram versus grep cases

| # | Collection | Filter | Matches | Bigram median ms | Grep median ms | Grep candidates |
| ---: | --- | --- | ---: | ---: | ---: | ---: |
| 1 | contents | `preferred_title ~ :ignorecase('almanach')` | 909 | 5.21 | 416.41 | 133,948 |
| 2 | contents | `preferred_title ~ :ignorecase('Liebe')` | 2,718 | 14.29 | 417.45 | 133,948 |
| 3 | contents | `preferred_title ~ :ignorecase('Taschen%')` | 618 | 7.66 | 415.96 | 133,948 |
| 4 | contents | `preferred_title ~ :ignorecase('Gedicht')` | 1,977 | 49.21 | 414.46 | 133,948 |
| 5 | contents | `preferred_title ~ :ignorecase('Mond')` | 419 | 5.45 | 417.45 | 133,948 |
| 6 | contents | `preferred_title ~ :ignorecase('Schiller')` | 307 | 4.82 | 418.27 | 133,948 |
| 7 | contents | `preferred_title ~ :ignorecase('Goethe')` | 59 | 3.97 | 422.67 | 133,948 |
| 8 | contents | `preferred_title = 'Der Mond'` | 12 | 4.73 | 385.35 | 542 |
| 9 | contents | `preferred_title = 'No. 11.'` | 4 | 13.22 | 392.61 | 10,947 |
| 10 | contents | `musenalm_type ?= 'Gedicht/Lied'` | 59,586 | 43.63 | 392.00 | 59,903 |
| 11 | contents | `year = 1800` | 2,908 | 1.72 | 412.89 | 133,948 |
| 12 | contents | `year = 1801` | 2,608 | 1.74 | 412.50 | 133,948 |
| 13 | contents | `year = 1802` | 3,073 | 1.77 | 416.63 | 133,948 |
| 14 | contents | `pseudonym = true` | 11,672 | 1.78 | 385.87 | 11,675 |
| 15 | contents | `pseudonym = false` | 122,276 | 2.15 | 407.11 | 132,702 |
| 16 | contents | `preferred_title ~ :ignorecase('Liebe') && year = 1800` | 57 | 3.41 | 418.37 | 133,948 |

## All 30 full-text cases

Case times are the median of the three per-process medians. Candidate, scan, parse, and match counts were stable across processes.

| # | Mode | Query / field | Matches | FTS candidates | Scans | Parses | Median ms |
| ---: | --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | filter | `annotation:fts ~ 'Goet'` | 293 | 637 | 637 | 638 | 10.33 |
| 2 | filter | `annotation:fts ~ 'Schill'` | 747 | 2,415 | 2,415 | 2,416 | 14.27 |
| 3 | filter | `annotation:fts ~ 'Mond'` | 45 | 2,000 | 2,000 | 2,001 | 13.58 |
| 4 | filter | `annotation:fts ~ :ignorecase('GOET')` | 293 | 637 | 637 | 638 | 11.66 |
| 5 | filter | `annotation:fts ~ 'Liebe'` | 1,142 | 12,954 | 12,954 | 12,955 | 31.26 |
| 6 | filter | `year = 1822 && annotation:fts ~ 'Goet'` | 8 | 637 | 10 | 11 | 2.18 |
| 7 | filter | `year >= 1800 && annotation:fts ~ 'Wien'` | 162 | 6,071 | 4,160 | 4,161 | 16.29 |
| 8 | filter | `year < 1800 && annotation:fts ~ 'Berlin'` | 146 | 4,899 | 1,499 | 1,500 | 9.57 |
| 9 | filter | `annotation:fts ~ 'Heinrich' \|\| year = 1822` | 2,394 | 0 | 133,948 | 133,949 | 83.52 |
| 10 | filter | `annotation:fts !~ 'Goet'` | 133,655 | 0 | 133,948 | 133,949 | 80.83 |
| 11 | filter | `preferred_title:fts ~ 'Goethe'` | 56 | 0 | 133,948 | 133,949 | 76.31 |
| 12 | filter | `preferred_title:fts ~ 'Schiller'` | 295 | 0 | 133,948 | 133,949 | 77.18 |
| 13 | filter | `preferred_title:fts ~ :ignorecase('goethe')` | 59 | 0 | 133,948 | 133,949 | 80.00 |
| 14 | filter | `preferred_title:fts ~ 'Gedicht'` | 1,810 | 0 | 133,948 | 133,949 | 75.91 |
| 15 | filter | `preferred_title:fts ~ 'A%buch'` | 12 | 0 | 133,948 | 133,949 | 76.59 |
| 16 | filter | `title_statement:fts ~ 'Verlag'` | 605 | 1,878 | 1,878 | 1,879 | 14.39 |
| 17 | filter | `title_statement:fts ~ :ignorecase('berlin')` | 759 | 4,899 | 4,899 | 4,900 | 19.62 |
| 18 | filter | `contributors.annotation:fts ?~ 'Goet'` | 0 | 637 | 637 | 637 | 10.62 |
| 19 | filter | `annotation:fts = 'Goethe'` | 0 | 0 | 133,948 | 133,948 | 80.41 |
| 20 | filter | `annotation:fts ~ 'XYZunlikely'` | 0 | 0 | 0 | 0 | 2.74 |
| 21 | word | `Goethe · annotation` | 284 | 461 | 461 | 462 | 1.76 |
| 22 | word | `Schiller · annotation` | 738 | 1,440 | 1,440 | 1,441 | 3.59 |
| 23 | word | `Goethe · preferred_title` | 59 | 461 | 461 | 462 | 1.20 |
| 24 | word | `Liebe · preferred_title` | 2,718 | 12,954 | 12,954 | 12,955 | 22.72 |
| 25 | word | `Goethe` | 446 | 461 | 461 | 462 | 1.67 |
| 26 | regex_clean | `Goet.*he · annotation` | 284 | 603 | 603 | 604 | 1.71 |
| 27 | regex_clean | `Schill · preferred_title` | 322 | 2,415 | 2,415 | 2,416 | 5.32 |
| 28 | regex_raw | `\*\*[^*]+\*\* · annotation` | 3 | 1,143 | 1,143 | 1,144 | 2.45 |
| 29 | regex_raw | `<[^>]+> · title_statement` | 0 | 133,948 | 133,948 | 133,948 | 174.99 |
| 30 | regex_clean | `Wien\|Berlin` | 3,490 | 10,630 | 10,630 | 10,631 | 19.18 |

## Raw results and commands

- [JSON with schema](json-with-schema.ndjson)
- [JSON without schema](json-without-schema.ndjson)
- [Bigram versus grep](grep-comparison.ndjson)
- Full text: [run 1](fts-run-1.ndjson), [run 2](fts-run-2.ndjson), [run 3](fts-run-3.ndjson)

```sh
cargo build --release -p objekte-bench --bins
target/release/json-query-mix --repo examples/musenalm --runs 7
target/release/json-query-mix --repo examples/musenalm --runs 7 --no-schema
target/release/json-query-grep-compare --repo examples/musenalm --workload crates/bench/workloads/musenalm-grep.json --runs 3
target/release/fts-query-mix examples/musenalm 3  # repeat in three fresh processes
```
