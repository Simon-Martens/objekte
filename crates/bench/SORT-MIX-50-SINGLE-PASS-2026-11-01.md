# Single-pass sorted JSON scan — 2026-11-01

This reruns the [50-query sorting benchmark](SORT-MIX-50-2026-11-01.md) after changing the in-memory sorted JSON path to reuse the document from candidate verification. It builds complete records and validates them only when they enter the retained sorted page. The same release-build runner used a warm repository handle, 25-record pages, a 10,000-result cap, one untimed pass, and three alternating timed rounds on an Intel Core i9-13900K with 30 GiB RAM. The OS page cache was warm and uncontrolled. Source files and schemas were unchanged.

## Results

The median sums add the 50 per-case medians; they are not one round’s wall time. Wall includes index warmup, the untimed pass, and all three timed rounds. RSS is peak process memory.

| Scope | Sorted median sum, before → after | Speedup | Sorted source reads, before → after | Wall, before → after | Peak RSS, before → after |
|---|---:|---:|---:|---:|---:|
| relevant | 35.223 → 7.219 s | 4.88× | 1,939,077 → 1,143,593 | 145.1 → 35.0 s | 294 → 349 MiB |
| original | 71.453 → 14.361 s | 4.98× | 3,934,074 → 2,270,232 | 292.1 → 66.9 s | 258 → 330 MiB |

All 50 capped totals and truncation flags matched the previous run in both scopes. The runner also checked stable totals and first-page IDs across its untimed pass and timed rounds. Focused tests compare full sorted records with direct execution, including hashes and validation state. The higher peak RSS is consistent with keeping parsed match documents alive through a parallel batch instead of rereading them.

An independent release CLI check of broad case #22 against the direct backend returned identical 25 complete records, capped total, and truncation flag. The optimized indexed-memory path read and parsed each of the 133,948 content files once in that check.

The largest relevant-collection improvements were `annotation DESC` (#22), **5,588 → 1,178 ms**; `created DESC` (#14), **5,218 → 1,010 ms**; and `year DESC` (#19), **5,174 → 812 ms**. The broad unscoped title-sort case #2 fell from **6,040 → 1,286 ms**. Query #22 now reads 133,948 source files rather than 267,622.

Broad unindexed sorts still scan their candidate sets to find the first page. In the original unscoped mode, a sort index present only in `contents` cannot stop a query early across all seven collections. Declaring single-field sort indexes for frequently requested fields such as `year` is a separate tradeoff in build time, memory, and snapshot size.

## All 50 sorted-query medians

Times are milliseconds. Relevant mode searches the listed collection; original mode searches all collections. `10k+` is a capped total with additional matches.

| # | Collection | Sort order | Hits | Relevant before → after | Original before → after |
|---:|---|---|---:|---:|---:|
| 1 | `contents` | `annotation ASC` | 45 | 7.7 → 5.3 | 14.2 → 13.6 |
| 2 | `contents` | `preferred_title ASC` | 10k+ | 242.9 → 115.0 | 6040.3 → 1286.1 |
| 3 | `contents` | `year DESC` | 51 | 8.7 → 6.6 | 37.8 → 31.3 |
| 4 | `contents` | `preferred_title DESC` | 10k+ | 264.5 → 119.8 | 6161.3 → 1319.1 |
| 5 | `contents` | `year ASC && preferred_title ASC` | 10k+ | 232.4 → 111.8 | 6359.3 → 1283.3 |
| 6 | `contents` | `numbering ASC` | 0 | 0.6 → 0.6 | 3.8 → 3.7 |
| 7 | `contents` | `year DESC && preferred_title ASC` | 2,908 | 112.4 → 21.0 | 122.6 → 30.0 |
| 8 | `contents` | `preferred_title ASC` | 10k+ | 222.4 → 106.6 | 5620.3 → 1208.1 |
| 9 | `contents` | `preferred_title DESC` | 10k+ | 240.6 → 105.1 | 4067.7 → 771.9 |
| 10 | `contents` | `year ASC && preferred_title ASC` | 10k+ | 552.7 → 86.0 | 1635.9 → 352.3 |
| 11 | `contents` | `numbering DESC` | 10k+ | 551.5 → 108.0 | 583.9 → 85.3 |
| 12 | `contents` | `@id DESC` | 5,516 | 290.3 → 66.0 | 287.3 → 30.2 |
| 13 | `contents` | `pseudonym ASC` | 10k+ | 473.0 → 103.9 | 480.5 → 65.1 |
| 14 | `contents` | `created DESC` | 10k+ | 5218.5 → 1010.3 | 5440.2 → 1016.0 |
| 15 | `contents` | `place_statement ASC` | 10k+ | 2757.9 → 465.2 | 2639.0 → 324.1 |
| 16 | `contents` | `year ASC` | 219 | 7.6 → 2.5 | 10.9 → 5.4 |
| 17 | `contents` | `preferred_title DESC` | 379 | 14.5 → 5.1 | 16.1 → 8.5 |
| 18 | `contents` | `numbering ASC` | 95 | 4.8 → 2.9 | 13.6 → 9.9 |
| 19 | `contents` | `year DESC` | 10k+ | 5173.9 → 811.8 | 5129.6 → 826.5 |
| 20 | `contents` | `preferred_title ASC` | 909 | 56.4 → 19.7 | 162.6 → 35.4 |
| 21 | `contents` | `title_statement ASC` | 274 | 21.4 → 8.4 | 26.7 → 10.7 |
| 22 | `contents` | `annotation DESC` | 10k+ | 5588.4 → 1178.0 | 5829.4 → 1099.6 |
| 23 | `contents` | `publication_statement ASC` | 618 | 56.7 → 15.6 | 132.6 → 52.8 |
| 24 | `contents` | `preferred_title DESC` | 909 | 53.9 → 15.3 | 119.1 → 33.9 |
| 25 | `contents` | `variant_title ASC` | 12 | 11.5 → 5.2 | 19.1 → 16.8 |
| 26 | `contents` | `musenalm_id ASC` | 10k+ | 4062.0 → 834.2 | 4202.7 → 676.8 |
| 27 | `agents` | `name ASC` | 1 | 14.1 → 10.1 | 15.9 → 14.1 |
| 28 | `agents` | `profession ASC` | 1,520 | 164.3 → 38.6 | 160.1 → 37.8 |
| 29 | `agents` | `corporate_body DESC` | 753 | 10.1 → 3.7 | 14.0 → 5.8 |
| 30 | `agents` | `created ASC` | 2,688 | 298.5 → 75.9 | 683.3 → 354.3 |
| 31 | `contents` | `preferred_title ASC` | 0 | 32.3 → 33.6 | 223.1 → 96.9 |
| 32 | `contents` | `updated ASC` | 0 | 133.6 → 138.7 | 142.6 → 154.2 |
| 33 | `contents` | `musenalm_id DESC` | 0 | 25.7 → 27.7 | 226.6 → 72.1 |
| 34 | `entries` | `preferred_title ASC` | 3,913 | 186.3 → 42.3 | 199.6 → 54.8 |
| 35 | `contents` | `entry ASC` | 81 | 5.5 → 3.2 | 12.5 → 11.1 |
| 36 | `contents` | `year ASC && preferred_title ASC` | 10k+ | 241.6 → 110.2 | 4845.3 → 977.6 |
| 37 | `contents` | `place_statement ASC` | 0 | 1.7 → 1.8 | 48.0 → 22.8 |
| 38 | `contents` | `title_statement ASC` | 0 | 0.7 → 0.8 | 196.7 → 69.1 |
| 39 | `contents` | `responsibility_statement ASC` | 121 | 6.5 → 3.8 | 13.4 → 12.5 |
| 40 | `contents` | `preferred_title DESC` | 10k+ | 523.5 → 93.4 | 2434.2 → 450.7 |
| 41 | `contents` | `year ASC` | 10k+ | 2834.6 → 502.3 | 2634.2 → 467.1 |
| 42 | `contents` | `annotation ASC` | 10k+ | 4348.5 → 750.0 | 4116.5 → 689.4 |
| 43 | `series` | `title ASC` | 341 | 25.0 → 6.2 | 19.4 → 10.5 |
| 44 | `series` | `frequency DESC` | 1 | 4.5 → 2.4 | 11.4 → 10.9 |
| 45 | `items` | `owner ASC` | 4,176 | 121.7 → 29.1 | 113.4 → 56.2 |
| 46 | `items` | `condition DESC` | 46 | 7.2 → 5.8 | 11.0 → 11.7 |
| 47 | `items` | `basis ASC` | 3 | 1.8 → 1.9 | 4.8 → 5.3 |
| 48 | `places` | `name ASC` | 7 | 2.4 → 1.4 | 3.4 → 3.5 |
| 49 | `pages` | `key ASC` | 1 | 3.0 → 2.9 | 6.3 → 6.9 |
| 50 | `pages` | `data.description ASC` | 7 | 2.6 → 3.9 | 160.9 → 169.5 |

Raw reruns: [relevant](results/musenalm-sort-mix-50-relevant-single-pass-2026-11-01.ndjson) and [original](results/musenalm-sort-mix-50-original-single-pass-2026-11-01.ndjson). The [previous raw results](SORT-MIX-50-2026-11-01.md) and [benchmark commands](README.md#sorting-the-50-query-workload) remain available.
