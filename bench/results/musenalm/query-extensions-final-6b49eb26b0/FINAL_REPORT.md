# Query extensions: final verification and timing

The measured core and feature harness are commit `6b49eb26b03117930b8c47b7885b126988dda21e`. The comparable pre-feature code is `e9d22a405c151dca1a3a8c5d87a39cb9de708db2`. The only commits after the independent oracle and optimized/reference acceptance at `96b582db592052ebf4e3b759bca329d4d786d02d` changed the feature benchmark example and its formatting; production core code is identical.

## Correctness

- `cargo test --workspace --all-targets`: passed, including independent JSON relation fixtures, non-JSON reader, caches, snapshots, regex candidate properties, TUI and CLI tests. `cargo clippy --workspace --all-targets` passed with advisory warnings. Scoped Rust formatting and `git diff --check` passed.
- The independent `jq` oracle checked complete record-ID sets for all 150 established queries in both alphabets: **300/300 passed** across 148,486 source files. Its 150 handwritten predicates do not use the core parser, binder, scanner or indexes. The full gzip memberships, per-case digests, manifest and report are in `../query-extensions-final-96b582db59/jq-oracle/`.
- The optimized and independent reference routes agreed on complete results, counts, pages, payloads and evidence for **150/150 ASCII** and **150/150 Unicode** acceptance cases. Logs are in `../query-extensions-final-96b582db59/accept-{ascii,unicode}.log`.
- The final fixed workload completed **3,000/3,000 timed samples**, with no failures or cross-profile answer differences. All 600 case/profile rows have the same counts and page sizes as the pre-feature recording. The separate feature workload checked 21 specified answers against the reference route in four profiles, then recorded 420 timed samples.

## Matched 150-query workload

Both recordings used Rust 1.99.1 release on the i9-13900K with the `performance` governor, CPUs 0–31, 32 query workers, zero source/decoded-cache budgets, both alphabets, disabled/64 MiB warm verified caches and five repetitions. The copied Musenalm corpus, schema manifest, resolved 150 queries and semantic registrations match. The raw workload fingerprints differ only because the example hashes the absolute checkout path along with manifest bytes: the tracked manifest Git blob is `39dcfe72d4e735cd4da392381517a62bcc493a8b` at both revisions. The comparator validates that blob, resolved queries, fingerprints, registrations, profiles, counts and pages.

| Profile | Before sum of medians, ms | Final sum of medians, ms | Difference, ms | Difference | Cases at least 1 ms slower |
| --- | ---: | ---: | ---: | ---: | ---: |
| ASCII, cache off | 1,654.295 | 1,361.156 | −293.140 | −17.7% | 1 |
| ASCII, warm | 1.548 | 1.237 | −0.311 | −20.1% | 0 |
| Unicode, cache off | 1,606.305 | 1,356.670 | −249.635 | −15.5% | 0 |
| Unicode, warm | 1.570 | 1.337 | −0.233 | −14.8% | 0 |

These sums describe one protocol-matched recording, not a general speedup guarantee. The filesystem cache was not flushed; the baseline's first ASCII discovery was much colder. Warm-cache differences are fractions of a millisecond across 150 queries. **No fixed-workload case changed candidate count, source opens or scanned-record count** in any profile, so this workload shows infrastructure cost with the new features unused, not new-feature selectivity. The full 600-row table, with exact query/options, absolute milliseconds, candidates, source opens, scans, matches and page sizes, is in `comparison/COMPARISON.md` and `comparison/comparison.csv`; it was also printed to `comparison-console.log`. Raw samples and plans are in `measurements.jsonl` and `query-plans.jsonl`.

ASCII Q096 (`contributors.type ?= "Autor:in"`) rose from 2.415 to 3.947 ms at the median, with the same 70,390 candidates and zero source opens/scans. Its five baseline samples were 6.179, 5.005, 2.301, 2.415 and 2.285 ms; final samples were 6.558, 5.494, 3.947, 2.621 and 2.389 ms. The Unicode median fell from 2.698 to 2.537 ms. This isolated, variable ASCII difference is unresolved, and the counters do not support a candidate-growth explanation. Q100 increased by 0.986 ms in ASCII with unchanged work; no other uncached case increased by 1 ms.

## Construction, restore and memory

| Alphabet | State | Build ms | Discovery ms | Extraction/index ms | Finalization ms | Snapshot write ms | Restore median ms | Snapshot bytes | Resident MiB | Peak MiB |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ASCII | Before | 3,998.572 | 2,922.431 | 800.099 | 262.302 | 324.780 | 494.530 | 111,214,033 | 459.1 | 545.3 |
| ASCII | Final | 973.213 | 211.615 | 510.141 | 238.550 | 243.514 | 457.728 | 111,214,089 | 460.7 | 547.2 |
| Unicode | Before | 1,187.038 | 228.307 | 667.581 | 281.723 | 330.940 | 501.668 | 113,724,609 | 679.9 | 744.2 |
| Unicode | Final | 1,068.403 | 208.332 | 605.714 | 243.888 | 276.457 | 473.518 | 113,724,665 | 841.0 | 907.0 |

Each build is one sample; each fresh-process restore median uses five samples and parses zero records. Both builds parse exactly 148,486 records. The snapshot grew by 56 bytes in each alphabet. Both recordings have 15 bigram stores, the same 25,477 ASCII and 37,909 Unicode gram keys, and identical estimated index/inventory bytes. The large first ASCII build difference is dominated by discovery and is consistent with different filesystem-cache conditions; it cannot be attributed to these query features. Unicode resident/peak RSS is higher in the final sequential two-alphabet process, while fresh restore RSS remains close (about 238 MiB versus 241 MiB). The source of this process-level difference is unresolved; the reported index-size counters do not show new resident index content on Musenalm.

## New feature workload

The fixed nine-record fixture has two declared relations, eleven edges and 736 reported resident edge bytes. `FEATURE_REPORT.md` and `feature-summary.csv` contain all 84 case/profile median rows; the four `feature-*.jsonl` files retain exact expressions, parameters, options, expected pages, plans, work counters and 420 raw samples. `feature-manifest.json` records the four exact invocations and timing boundary. These are first measurements and have no pre-feature equivalents. Search times are mostly microseconds and should not be read as stable speed ratios.

The target-first relation case returned two matches with zero source scans and zero source opens. The source-restricted case visited one edge and scanned zero source records. Selected FTS with `contributors[0].title` retained positional semantics and used residual verification (six source scans, three target reads in the ASCII uncached median). The combined regex-pattern parameter, typed ID parameter, selected relation FTS and inline sort case returned two exact matches with one target read and zero source scans. Positive forward semijoins use the existing exact posting/relation paths; OR, NOT, positions, backlinks and unsupported universal-array paths use the verified residual route. Reader-owned segment readers cannot expose array occurrence provenance, so positional selected FTS is explicitly rejected for those readers.

The integration branch is `integration/typed-query-extensions`; this report does not merge it into development or master. The main checkout and the user's TUI Musenalm corpus were not modified by the benchmark.
