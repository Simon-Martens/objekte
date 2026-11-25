# Musenalm: Objekte versus Tantivy

Measured on 2026-11-24 using release code revision `0f2e6adfa8e3140f8492c1e835efe97d69942a8c` and Tantivy 0.27.0. The later `01cb6c3322` commit adds regression tests only. The [full per-case report](../../../../bench/results/musenalm/tantivy-full-0f2e6adf-20261124/REPORT.md), [run manifest](../../../../bench/results/musenalm/tantivy-full-0f2e6adf-20261124/run-manifest.json), [inventory](../../../../bench/results/musenalm/tantivy-full-0f2e6adf-20261124/compatibility-inventory.json), [verification](../../../../bench/results/musenalm/tantivy-full-0f2e6adf-20261124/verification.json), [raw samples](../../../../bench/results/musenalm/tantivy-full-0f2e6adf-20261124/objekte-historical-samples.json), and [compact evidence](evidence.json) retain the underlying observations. The result directory also has the Tantivy, extended-mode, and aggregate samples.

## Reproduce

From the integration worktree, with the same physical fixture path:

```sh
python3 bench/run_musenalm_tantivy.py full --root /home/simon/source/objekte/examples/musenalm --result-dir bench/results/musenalm/tantivy-full-reproduction
```

The result directory must be fresh. See [benchmark README](../../../../bench/tantivy/README.md) for the quick command and overrides. The run manifest records the corpus, schema, workload and lockfile hashes; rustc and host; 32-CPU affinity; one build and query worker per engine; 100 MB Tantivy writer budget; zero Objekte verified-result cache; warm source-file cache; and the sequential engine order (Objekte then Tantivy). Every timed request had one warm-up and seven samples. Build cost has **one observation** per engine and schema mode.

## Correctness and coverage

The seven collections contain 148,486 records, including 133,948 `contents` records. Both engines saw the same complete identity universe and collection counts. For every admitted predicate, verification compared the complete matching `(collection, record_id)` set and rejected duplicate identities. For every admitted page, it compared ordered locator tuples, including deep offsets. No verification failed. The separate [independent jq oracle](../../../../bench/results/musenalm/tantivy-jq-20261124/REPORT.md) passed 300/300 historical ASCII and Unicode membership comparisons; its metadata fingerprint matches this run (`eec50dbfbfd023aad31e9c3609f4a7f14b177ecc1a022c182321ae704c188cf6`).

| Cases | Predicate count verified | Locator page verified | Unsupported predicates | Unsupported pages |
| --- | ---: | ---: | ---: | ---: |
| Q001–Q150 | 104 | 94 | 46 | 56 |
| Q151–Q220 | 4 | 0 | 66 | 70 |
| Total | 108 | 94 | 112 | 126 |

Aliases reduce these to 94 distinct measured counts and 92 distinct pages. Q134–Q136 share one count request but use separate page offsets; Q137–Q139 collapse to one locator projection. Unsupported mappings and reasons remain visible per case in the inventory. In particular, text and compound sorts, complex string operations, metadata facts, relation traversal and several extended forms are excluded. Count compatibility does not imply page compatibility. Q137 and Q138 page timings describe locators, not bytes or logical records; evidence is projected away for every page.

## Construction

| Schema | Engine | Build time | Peak RSS | RSS ready | Segments | Managed index bytes |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Historical | Objekte | 3.493 s | 451.0 MiB | 407.8 MiB | 15 | unavailable |
| Historical | Tantivy | 6.487 s | 402.5 MiB | 394.2 MiB | 5 | 180.3 MiB |
| Extended | Objekte | 3.651 s | 503.5 MiB | 460.6 MiB | 15 | unavailable |
| Extended | Tantivy | 6.435 s | 393.9 MiB | 385.5 MiB | 5 | 180.3 MiB |

Objekte uses its ordinary in-memory index with the historical or extended Musenalm registration and the default ASCII alphabet. Tantivy uses a RAM directory, one document per source record, indexed/stored/fast typed JSON, exact collection and ID terms, derived global and path-scoped string terms, a numeric fast sort field, and an identity ordinal fast field. Each decoded string is indexed as overlapping chunks of at most 1,024 Unicode scalars; case-sensitive literal search runs a regex over those term dictionaries. Tantivy's materialized fields can answer some operations that Objekte reads from source. Its measured construction was about 1.8 times longer in both modes. Objekte's public in-memory API does not expose a comparable byte size, so the 180.3 MiB figure is only Tantivy's managed RAM-directory segment-file size. Peak and ready RSS are process measurements and include more than the index.

## Selected query medians

All times include the contracted search and, for pages, retrieval of winning locators. Ratio is Tantivy / Objekte; values above 1 favor Objekte. The [full report](../../../../bench/results/musenalm/tantivy-full-0f2e6adf-20261124/REPORT.md) lists all 186 rows with min–max variability.

| Case and contract | Matches | Objekte | Tantivy | Ratio |
| --- | ---: | ---: | ---: | ---: |
| Q001 `Almanach` count | 3,269 | 39.50 ms | 260.43 ms | 6.59 |
| Q006 `ü` count | 65,548 | 790.41 ms | 266.98 ms | 0.338 |
| Q007 `a` count | 147,798 | 795.78 ms | 295.29 ms | 0.371 |
| Q009 collection-scoped `Leipzig` count | 1 | 0.015 ms | 269.90 ms | 17,658 |
| Q030 field-scoped `Taschenbuch` count | 1,264 | 9.84 ms | 53.35 ms | 5.42 |
| Q044 integer year count | 60 | 0.013 ms | 0.010 ms | 0.793 |
| Q044 locator page | 60 | 0.041 ms | 0.470 ms | 11.4 |
| Q054 fractional range count | 66 | 1.619 ms | 1.036 ms | 0.640 |
| Q081 repeated path membership count | 888 | 3.765 ms | 0.011 ms | 0.00294 |
| Q134 numeric page, offset 0 | 133,948 | 0.376 ms | 2.204 ms | 5.86 |
| Q135 numeric page, offset 1,000 | 133,948 | 0.904 ms | 3.014 ms | 3.34 |
| Q136 numeric page, offset 10,000 | 133,948 | 5.695 ms | 5.258 ms | 0.923 |

Literal FTS shows the strongest case-dependent tradeoff. A large one-character match set favors Tantivy; a selective scoped literal strongly favors Objekte. Tantivy's term-dictionary regex takes roughly 260–270 ms even for many selective or empty literal cases. That fixed scan cost is an inference from the repeated timings and the implementation, not an isolated profiler result. The near-equal Q136 page result and other small differences should not be treated as stable engine rankings from this single run. No aggregate winner is reported.

The numeric-domain diagnostic scanned 424,708 numeric leaves across ten paths: it found no distinct decimal values collapsing to the same `f64` bits within a path and no integers above 2^53 in this corpus. This supports the reported numeric equality and ordering mapping for this fixture; the diagnostic is in `numeric-domain.json` in the result directory. It does not prove equivalence for arbitrary future datasets.

## Validation and limits

`cargo test --workspace --locked` passed, as did both benchmark fixture tests (literal FTS and structured/numeric edge cases). The independent jq oracle passed 300/300. The full comparison exited successfully with seven samples for every measured contract. No core behavior was changed, so the full comparison is the relevant performance benchmark; there is no comparable base-revision Tantivy benchmark. One build observation and one host limit construction conclusions. The published release run predates a test-only commit, with no measured production-code change after it.
