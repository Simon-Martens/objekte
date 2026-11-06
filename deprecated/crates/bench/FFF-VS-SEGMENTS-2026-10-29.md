# FFF original-file search versus segment cache — 29 October 2026

This is a warm, repeated-query comparison on Musenalm `contents` (133,948 original JSON files). The benchmark uses [FFF Search](https://github.com/dmtrKovalenko/fff) at commit `d1c7dd47130ca670ab71e7466a1a3b4a57d5348d`, with its default Rust walker, in-memory bigram content index, mmap cache, and a 5,000-file/128 MB content-cache budget. FFF indexed all 133,948 files. The FFF initial scan and index took 294 ms and 303 ms in the two main runs; these setup times are excluded from query timings. The existing segment cache was warm and held open in the same process.

The [33-case workload](workloads/musenalm-fff.json) covers title, variant title, incipit, responsibility, annotation, publication, array, number, and boolean fields. It includes equality, LIKE-style wildcards, three regex **candidate searches**, and combined text/numeric or text/boolean filters. `~` in the filter language is LIKE-style matching, not a regex operator. Each case was warmed, then timed five times in alternating backend order. A second independent process ran the cases in reverse order. Medians below are milliseconds. The OS page cache was warm but uncontrolled.

Both paths count all matches and return the first three IDs. The segment path uses the production `Repository::query_json` API. The FFF path searches the original JSON files for candidates, reads each candidate in parallel, checks metadata before and after, computes SHA-256, parses with Sonic, applies the same compiled `JsonFilter`, sorts matching IDs, and rereads the first three. Full counts and first-three IDs agreed for every case on warm-up and every timed run. The segment path also validates the returned three records against JSON Schema; the FFF prototype does not. FFF's file open in this benchmark lacks the production path's `openat`/no-follow protection. Thus these numbers are a matching performance comparison, not an implementation-ready parity claim.

## Candidate strategies

The primary FFF strategy searches with a hand-specified literal or regex necessary for each filter and also includes every file containing a backslash escape. Only 90 source files contain a backslash. FFF found these in 51–52 ms once at setup; this is excluded from repeated-query timings. The union is conservative for the ASCII patterns in this workload: any matching JSON string whose raw bytes break the literal contains a backslash and remains a candidate. A production implementation would have to derive the hints automatically and maintain the escape-file set when files change.

A separate run used the filter compiler's fully escaped raw-JSON regex, which handles `\u` spellings without the extra escape-file set. It won only **1 of 33** cases; its median of per-case medians was **61.6 ms**, versus **11.5 ms** for segments in that run. Its regex cannot exploit FFF's plain-literal fast path well. The primary escape-union prototype is the stronger FFF configuration for repeated queries here.

## Summary

| Measure | FFF escape union | Segments |
| --- | ---: | ---: |
| Forward run: median of 33 per-case medians | 25.8 ms | 12.7 ms |
| Reverse run: median of 33 per-case medians | 24.2 ms | 11.9 ms |
| Cases won, forward / reverse | 11 / 12 | 22 / 21 |

These aggregate medians describe this chosen workload; they are not a weighting of user query frequency. FFF is particularly strong where its hint is much more selective than the current values-only prefilter, such as `preferred_title = 'No. 11.'` (101 versus 16,141 candidates), and where the segment planner builds a broad numeric ID set before checking a small text candidate set. Segments lead on most ordinary text searches and strongly on selective typed lookups. A broad `numbering >= 10` range is an exception: parsing all files with FFF is faster than materializing and verifying the large segment index result.

## All queries

`FFF` and `Segments` are the forward-run medians; `FFF rev` and `Segments rev` are the reverse-run medians. Candidate counts are from the forward run. Full filter strings and candidate hints are in the workload file.

| Case | Matches | FFF candidates | Segment text candidates | FFF | Segments | FFF rev | Segments rev |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `title_almanach` | 909 | 2,199 | 2,109 | 13.5 | 7.4 | 12.4 | 6.3 |
| `title_liebe` | 2,718 | 7,805 | 7,716 | 26.5 | 13.7 | 21.9 | 11.3 |
| `title_goethe` | 59 | 536 | 446 | 3.2 | 3.9 | 1.6 | 2.7 |
| `title_schiller` | 307 | 1,236 | 1,146 | 7.1 | 5.9 | 3.4 | 4.1 |
| `title_gedicht` | 1,977 | 66,060 | 65,996 | 108.4 | 55.7 | 109.2 | 58.7 |
| `title_mond` | 419 | 1,347 | 1,258 | 8.8 | 4.6 | 12.8 | 5.2 |
| `variant_liebe` | 0 | 7,805 | 7,716 | 24.5 | 12.7 | 20.8 | 11.9 |
| `title_statement_mond` | 398 | 1,347 | 1,258 | 9.8 | 4.9 | 11.1 | 5.5 |
| `incipit_liebe` | 4,733 | 7,805 | 7,716 | 25.8 | 12.1 | 24.2 | 11.4 |
| `responsibility_goethe` | 121 | 536 | 446 | 2.4 | 3.4 | 3.1 | 5.3 |
| `annotation_goethe` | 284 | 536 | 446 | 3.6 | 4.2 | 3.5 | 3.9 |
| `publication_leipzig` | 0 | 1,519 | 1,429 | 6.5 | 7.3 | 4.5 | 4.9 |
| `type_gedicht_lied` | 59,586 | 59,652 | 65,996 | 104.0 | 55.5 | 106.3 | 55.9 |
| `type_text` | 17,124 | 25,094 | 25,042 | 45.4 | 28.6 | 53.4 | 25.0 |
| `title_eq_mond` | 12 | 173 | 1,258 | 3.6 | 6.1 | 3.5 | 6.9 |
| `title_eq_issue` | 4 | 101 | 16,141 | 0.9 | 27.8 | 0.8 | 25.4 |
| `title_prefix_lieb` | 448 | 12,321 | 12,232 | 27.7 | 14.4 | 27.4 | 15.2 |
| `title_like_mond` | 419 | 1,347 | 1,258 | 11.5 | 5.1 | 9.0 | 4.7 |
| `title_like_wildcard` | 0 | 3,743 | 86,354 | 37.9 | 79.7 | 38.8 | 81.4 |
| `title_regex_or` | 3,128 | 8,951 | 8,863 | 26.3 | 11.3 | 27.5 | 12.2 |
| `year_eq_1800` | 2,908 | 133,948 | 0 | 74.4 | 4.4 | 75.3 | 4.5 |
| `year_lt_1800` | 38,795 | 133,948 | 0 | 76.1 | 45.3 | 74.9 | 44.4 |
| `pseudonym_true` | 11,672 | 11,763 | 11,675 | 24.1 | 18.2 | 26.1 | 21.0 |
| `pseudonym_false` | 122,276 | 132,703 | 132,702 | 208.4 | 153.2 | 214.4 | 149.9 |
| `numbering_ge_10` | 112,778 | 133,948 | 0 | 77.1 | 130.3 | 77.5 | 131.6 |
| `musenalm_id_eq` | 1 | 133,948 | 0 | 72.8 | 0.9 | 72.4 | 0.9 |
| `title_liebe_year` | 57 | 7,805 | 7,716 | 27.1 | 7.3 | 32.4 | 8.0 |
| `title_almanach_year` | 569 | 2,199 | 2,109 | 11.4 | 33.0 | 23.0 | 33.4 |
| `title_goethe_pseudonym` | 3 | 536 | 446 | 2.1 | 6.1 | 2.4 | 6.2 |
| `title_gedicht_old` | 291 | 66,060 | 65,996 | 109.2 | 44.0 | 113.9 | 44.8 |
| `type_gedicht_year` | 43,555 | 59,652 | 65,996 | 105.7 | 80.0 | 108.5 | 83.7 |
| `title_like_regex_prefix` | 56 | 81,542 | 86,354 | 130.1 | 79.4 | 137.3 | 81.5 |
| `mond_numbering` | 373 | 1,347 | 1,258 | 9.0 | 37.7 | 8.6 | 37.9 |

## Reproduce

```sh
cargo build --release --manifest-path crates/bench/fff-compare/Cargo.toml
crates/bench/fff-compare/target/release/objekte-fff-compare --repo examples/musenalm --workload crates/bench/workloads/musenalm-fff.json --data-dir target/segment-accept-final --runs 5 --prefilter escape-union
```

The independent benchmark package pins FFF to the cited commit. Use `--prefilter safe` for the compiler-generated raw-JSON regex and `--prefilter manual` for the hand hints without escape coverage. The latter is a corpus-specific speed bound, not a generally sound implementation.
