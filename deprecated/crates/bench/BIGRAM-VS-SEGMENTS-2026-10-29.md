# In-memory bigram and typed index versus segment cache — 29 October 2026

Warm, repeated, full `Repository::query_json` queries on Musenalm `contents` (133,948 JSON files). Both backends ran each [43-case workload](workloads/musenalm-bigram.json) five timed times in alternating order. Each case was warmed first; a separate process repeated the workload in reverse order. Timings are medians in milliseconds. The OS page cache was warm but not controlled. Both paths counted every match and returned the first three records, including normal result validation.

The new `bigram` backend builds consecutive and skip-one FFF bigrams over decoded scalar values, plus in-memory ordered postings for declared numeric, boolean, and enum fields. Regex-to-bigram planning and simple conjunctive typed predicates narrow candidates. The backend then opens original files, applies a safe source regex, parses with Sonic, and evaluates the full filter. No index result is treated as a final match. The `segments` backend used the existing warm persistent cache.

## Result

| Measure | Bigram | Segments |
| --- | ---: | ---: |
| Forward: median of 43 per-case medians | 14.8 ms | 22.6 ms |
| Reverse: median of 43 per-case medians | 15.5 ms | 23.4 ms |
| Cases won, forward / reverse | 35 / 37 | 8 / 6 |

The first in-memory index build took 525 ms in the forward process and 520 ms in the reverse process; it is excluded from warm query timings. These aggregate medians describe this workload, not expected user query frequency.

For a single fresh CLI process running `year = 1800`, one measured run took 0.57 s and peaked at 218 MiB RSS with the bigram backend; the already-built segment cache took 0.32 s and peaked at 344 MiB RSS. These are one-shot observations with different setup paths, not a controlled memory comparison.

## Where time goes

The forward run had a median candidate-selection cost of 1.4 ms and a median source-scan/verification cost of 12.8 ms. Result materialization for the three returned records had a median of 0.1 ms. These are medians across different cases and should not be added together.

- `preferred_title ~ 'Liebe' && year = 1800`: typed and text postings intersect to 242 files; 125 pass the source regex. Bigram took 1.8 ms versus 7.4 ms for segments in the forward run.
- `preferred_title = 'No. 11.'`: the bigram postings still admit 16,141 files for common short fragments. The source regex reduces parsing to 1,577, but all 16,141 files must be opened and read. The source-scan stage alone took 14.8 ms; the complete query took 17.4 ms versus 6.4 ms for segments.
- `year = 1800`: ordered numeric postings reduce 133,948 files to 2,908. Bigram took 5.7 ms versus 6.4 ms in the forward run. Broad ranges and boolean values still require reading many matching files.
- `year = 1800 || year = 1900` and mixed text/number OR do not yet get a safe typed union plan. They scan all 133,948 files, taking about 79.9–86.9 ms. These cases are still faster than the measured segment path, but a union planner could improve them.
- Negative matches, `exists`, and empty-string comparisons can have no useful positive bigram. When they admit most files, a counted query necessarily verifies many sources; the observed broad scans were roughly 77–80 ms warm.

The detailed worker timers are sums across parallel workers, so they are CPU time rather than wall time. They show file open/read and Sonic parsing as the main source-stage costs; the `bigram_scan_ms` column below is the corresponding wall time.

## Correctness

The [release acceptance test](../core/tests/musenalm_bigram.rs) compared the **complete ordered ID list and total** from the bigram backend with direct source evaluation for every one of the 43 filters; all passed. The focused suite also checks Unicode escapes, regex candidate completeness on generated short strings, malformed JSON, array cardinality errors, exact decimal ordering, typed predicates, and targeted edits. This tests the corpus and listed filter shapes; it is not a formal proof for every possible filter expression.

## All queries

`Indexed` is the number of files selected by the in-memory indexes. `Parsed` is the number passing the source regex and parsed for exact evaluation. Forward and reverse columns are full-query medians.

| Case | Matches | Indexed | Parsed | Bigram fwd | Segments fwd | Bigram rev | Segments rev |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `title_almanach` | 909 | 2,411 | 2,109 | 4.2 | 6.1 | 6.0 | 9.1 |
| `title_liebe` | 2718 | 12,721 | 7,716 | 10.6 | 13.2 | 17.0 | 18.3 |
| `title_goethe` | 59 | 456 | 446 | 2.7 | 4.0 | 3.2 | 5.3 |
| `title_schiller` | 307 | 1,409 | 1,146 | 4.1 | 6.5 | 3.9 | 4.9 |
| `title_gedicht` | 1977 | 66,631 | 65,996 | 43.8 | 59.5 | 43.4 | 62.2 |
| `title_mond` | 419 | 5,216 | 1,258 | 5.6 | 5.3 | 9.0 | 7.2 |
| `variant_liebe` | 0 | 12,721 | 7,716 | 14.8 | 12.3 | 16.0 | 18.1 |
| `title_statement_mond` | 398 | 5,216 | 1,258 | 6.5 | 5.7 | 8.0 | 8.1 |
| `incipit_liebe` | 4733 | 12,721 | 7,716 | 12.9 | 13.1 | 16.7 | 17.8 |
| `responsibility_goethe` | 121 | 456 | 446 | 2.4 | 4.0 | 3.8 | 5.4 |
| `annotation_goethe` | 284 | 456 | 446 | 3.5 | 5.6 | 2.9 | 4.2 |
| `publication_leipzig` | 0 | 1,429 | 1,429 | 7.2 | 8.5 | 3.5 | 5.1 |
| `type_gedicht_lied` | 59586 | 60,895 | 59,903 | 40.3 | 55.9 | 41.0 | 57.1 |
| `type_text` | 17124 | 25,044 | 25,042 | 20.6 | 26.8 | 19.8 | 28.0 |
| `title_eq_mond` | 12 | 4,151 | 542 | 8.8 | 4.7 | 8.6 | 4.9 |
| `title_eq_issue` | 4 | 16,141 | 1,577 | 17.4 | 6.4 | 15.2 | 6.6 |
| `title_prefix_lieb` | 448 | 15,231 | 12,232 | 16.1 | 20.9 | 12.8 | 17.1 |
| `title_like_mond` | 419 | 5,216 | 1,258 | 6.3 | 6.1 | 7.7 | 6.2 |
| `title_like_wildcard` | 0 | 86,354 | 81,480 | 54.1 | 83.9 | 55.1 | 84.2 |
| `title_regex_or` | 3128 | 17,363 | 8,863 | 18.7 | 16.1 | 19.7 | 17.7 |
| `year_eq_1800` | 2908 | 2,908 | 2,908 | 5.7 | 6.4 | 6.0 | 6.5 |
| `year_lt_1800` | 38795 | 38,795 | 38,795 | 26.3 | 45.6 | 29.4 | 45.9 |
| `pseudonym_true` | 11672 | 11,672 | 11,672 | 14.4 | 22.6 | 14.3 | 23.4 |
| `pseudonym_false` | 122276 | 122,276 | 122,276 | 70.9 | 164.1 | 71.5 | 166.2 |
| `numbering_ge_10` | 112778 | 112,778 | 112,778 | 67.1 | 138.4 | 66.8 | 138.1 |
| `musenalm_id_eq` | 1 | 1 | 1 | 1.2 | 0.4 | 1.3 | 0.5 |
| `title_liebe_year` | 57 | 242 | 125 | 1.8 | 7.4 | 4.2 | 10.4 |
| `title_almanach_year` | 569 | 1,309 | 1,098 | 5.1 | 37.6 | 4.1 | 38.1 |
| `title_goethe_pseudonym` | 3 | 20 | 19 | 3.2 | 9.5 | 1.8 | 5.8 |
| `title_gedicht_old` | 291 | 17,638 | 17,454 | 22.9 | 52.9 | 15.5 | 52.2 |
| `type_gedicht_year` | 43555 | 44,475 | 43,701 | 32.8 | 79.5 | 33.1 | 85.7 |
| `title_like_regex_prefix` | 56 | 86,354 | 81,480 | 55.1 | 83.6 | 54.1 | 84.2 |
| `mond_numbering` | 373 | 4,422 | 4,422 | 6.9 | 39.4 | 6.8 | 40.6 |
| `year_ne_1800` | 131040 | 133,948 | 133,948 | 77.5 | 118.8 | 77.7 | 123.4 |
| `title_not_mond` | 133529 | 133,948 | 133,948 | 81.1 | 121.3 | 79.9 | 127.9 |
| `exists_title` | 133948 | 133,948 | 133,948 | 76.9 | 118.2 | 76.6 | 121.0 |
| `year_or` | 2908 | 133,948 | 133,948 | 79.9 | 123.8 | 79.6 | 121.8 |
| `text_or_year` | 3324 | 133,948 | 133,948 | 86.9 | 125.6 | 89.8 | 124.1 |
| `year_window` | 13769 | 13,769 | 13,769 | 12.0 | 59.6 | 13.4 | 57.9 |
| `year_and_bool` | 219 | 219 | 219 | 3.0 | 17.8 | 1.7 | 13.8 |
| `type_length` | 3350 | 133,948 | 133,948 | 78.6 | 121.9 | 78.4 | 120.4 |
| `title_empty` | 0 | 133,948 | 133,948 | 78.0 | 120.3 | 77.0 | 122.9 |
| `year_null` | 0 | 133,948 | 133,948 | 77.9 | 122.4 | 76.9 | 122.2 |

## Reproduce

```sh
cargo run --release -p objekte-core --example profile_bigram_query
cargo run --release -p objekte-core --example profile_bigram_query -- examples/musenalm target/segment-accept-final --reverse
cargo test --release -p objekte-core --test musenalm_bigram -- --ignored --nocapture
```
