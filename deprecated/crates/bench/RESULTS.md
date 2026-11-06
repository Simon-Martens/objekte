# Initial benchmark results

Measured on 2026-10-27 with an Intel Core i5-1145G7 (4 cores, 8 threads), Linux, release build, `--threads 4`, qj 0.3.0, and jaq-all 0.3.0. These are single local runs with the operating system's file cache uncontrolled. The disk runs put generated files and caches under `target/` on btrfs. An earlier run used `/tmp`, which is tmpfs on this machine. Timings are not stable performance guarantees.

| Collection | Records | Source bytes | NDJSON bytes | Index bytes |
| --- | ---: | ---: | ---: | ---: |
| Generated `items` | 200,000 | small synthetic objects | 22,822,223 | 29,106,413 |
| Musenalm `entries` | 4,928 | 180,435,434 | 133,520,460 | 804,087 |
| Lenz `sekundaerliteratur` | 2,133 | — | 944,770 | 356,509 |

## Disk-backed working directory

| Operation | 200k generated | Musenalm entries | Lenz secondary literature |
| --- | ---: | ---: | ---: |
| Generate synthetic source files | 9.186 s | — | — |
| Cold cache build | 2.819 s | 2.054 s | 0.034 s |
| Full rebuild, unchanged sources | 2.159 s | 2.358 s | 0.027 s |
| Scan unchanged sources | 1.093 s | 0.017 s | 0.006 s |
| Scan after one external change | 1.032 s | — | — |
| Refresh one known record's shard | 0.009 s | 0.039 s | 0.002 s |
| Open published generation | <0.001 s | <0.001 s | <0.001 s |
| Load full ID index | 0.285 s | 0.008 s | 0.002 s |

The generated source-file creation is excluded from cache build time. Its one-record refresh followed a real source edit. The example-repository refreshes rewrote an unchanged record's shard, leaving those repositories untouched.

| Filter and matching output count | 200k: qj / jaq | Musenalm: qj / jaq | Lenz: qj / jaq |
| --- | ---: | ---: | ---: |
| `.id` (200,000 / 4,928 / 2,133 outputs) | 0.087 / 0.252 s | 0.246 / 1.384 s | 0.002 / 0.006 s |
| `select(.data.year != null and .data.year < 1800) \| .id` (80,000 / 1,107 / 0 outputs) | 0.090 / 0.605 s | 0.317 / 1.647 s | 0.003 / 0.008 s |

## RAM-backed working directory

The same synthetic test under `/tmp` took 1.994 s for a cold build, 1.761 s for a full rebuild, 0.871 s for an unchanged source scan, 0.888 s for a changed source scan, and 0.008 s for a targeted shard refresh. Its qj/jaq query times were 0.060/0.226 s for `.id` and 0.076/0.527 s for the guarded year filter. The source-file generation took 2.023 s. The disk results above are a more relevant starting point for a persistent cache.

Both query engines used their Rust library APIs, parsed the same 64 shard files, compiled a filter per worker, and used four workers. This first filter benchmark counts outputs but does not serialize them or compare output values. qj's specialized tape evaluator was not invoked. The two engines agreed on output counts for these filters; qj was faster on both measured filters in this setup. Collection-wide sorting and grouping were measured separately below; joins, complete result serialization, and tape execution still need measurement before selecting a production engine.

The first index layout stored all 200,000 IDs in one 29 MB JSON file and took roughly 0.2–0.34 s for a targeted refresh. Splitting the index into 64 files reduced that refresh to 0.009 s in the final disk-backed run. The total index size remains large for small records. Opening the full index is slower than a simple view query at this scale, so view scans use the lightweight generation opener. Source scanning remains the major cost of detecting unknown third-party edits; the current detector compares path, size, and modification time. It cannot detect a same-size edit that preserves the timestamp.

The Musenalm repository contains additional JSON directories, including `contents`, that are not registered as collections in its current `.editor/repository.json`. This benchmark used the registered `entries` collection. The generated 200,000-record case tests high file count separately.

## Filtering, sorting, and grouping

Additional disk-backed release runs on 2026-10-27 used the same four-worker setting for independent-record filters. The global queries slurped all NDJSON records into one array and used one thread. Each global time below includes parsing that array, compiling and running the expression, and serializing the final result. The global result values matched between qj and jaq.

| Query | 200k generated: qj / jaq | Musenalm entries: qj / jaq |
| --- | ---: | ---: |
| Filter: `select(.data.year != null and .data.year < 1800) \| .id` | 0.094 / 0.504 s (80,000 rows) | 0.303 / 1.575 s (1,107 rows) |
| Sort: `sort_by(.data.year, .id) \| .[:100] \| map(.id)` | 0.780 / 1.385 s (100 rows) | 0.913 / 3.048 s (100 rows) |
| Group: `group_by(.data.year) \| map([.[0].data.year, length])` | 0.558 / 1.047 s (500 groups) | 0.646 / 2.598 s (166 groups) |
| Filter then sort: `map(select(.data.year != null and .data.year < 1800)) \| sort_by(.data.year, .id) \| .[:100] \| map(.id)` | 0.488 / 1.660 s (100 rows) | 0.604 / 2.433 s (100 rows) |

The global sort's parse and query phases were 0.242/0.538 s for qj and 0.456/0.929 s for jaq on 200,000 records. On Musenalm they were 0.700/0.213 s for qj and 2.189/0.858 s for jaq. These are single runs with uncontrolled operating-system file cache, so use them to choose the next experiment rather than as stable speed ratios. Collection-wide array queries have memory cost proportional to the collection; streaming or partitioned strategies need separate benchmarks.

## Exploratory database comparison: ID plus JSON only

On 2026-10-27, I compared embedded DuckDB 1.5.6 and SQLite 3.53.1 through their Python bindings. This was a quick native-engine experiment, not an integration into the Rust benchmark crate. Each collection table had only `id` and `data`; DuckDB stored `data` as JSON, while SQLite stored it as text JSON or binary JSONB. Both had an ID index. The same 64 NDJSON shards supplied the input. SQL used the built-in JSON operators/functions, with no extracted `year` column. Results for filter, sort, and group queries matched across engines, including explicit null ordering. Times below are medians of five warm runs on btrfs; they include fetching results into Python.

| Dataset and query | DuckDB JSON | SQLite text JSON | SQLite JSONB | SQLite JSONB with year expression index |
| --- | ---: | ---: | ---: | ---: |
| 200k: filter IDs (80,000 results) | 0.037 s | 0.089 s | 0.136 s | 0.047 s |
| 200k: sort, first 100 | 0.017 s | 0.063 s | 0.056 s | <0.001 s |
| 200k: group by year (500 groups) | 0.029 s | 0.105 s | 0.124 s | 0.014 s |
| Musenalm: filter IDs (1,107 results) | 0.094 s | 0.236 s | 0.037 s | <0.001 s |
| Musenalm: sort, first 100 | 0.121 s | 0.230 s | 0.035 s | <0.001 s |
| Musenalm: group by year (166 groups) | 0.173 s | 0.404 s | 0.215 s | <0.001 s |

The SQLite index was `CREATE INDEX year_id ON records(json_extract(data,'$.year'), id)`. It took 0.161 s to build on 200,000 small records and 0.044 s on Musenalm. Its benefit applies when a query uses the matching expression. Building an index for every possible user-defined field would add write cost and storage, so this is most suitable for saved or frequently used views. The indexed Musenalm measurements were taken after restoring the updated record to its original JSON.

Importing the already-built NDJSON cache into DuckDB took about 0.28 s including its ID index for the synthetic case and 0.42 s for Musenalm. SQLite JSONB import took about 1.26 s and 3.43 s, respectively, including its ID index; those SQLite numbers include a Python row-by-row JSON decoder and are **not** a fair Rust-ingestion comparison. They also exclude the source-file scan and NDJSON build. Single-ID updates were a few milliseconds in both engines, but DuckDB's Musenalm database temporarily doubled from about 136 MB to 272 MB after a checkpoint following one update; repeated checkpoints alternated those sizes in this experiment. SQLite JSONB occupied about 123 MB before its expression index. Disk size and update behavior deserve a longer lifecycle test.

The comparison is exploratory: the database engines were native libraries, but the harness used Python bindings, different ingestion paths, and uncontrolled file cache. It demonstrates that SQL over untyped JSON can be much faster than slurping and grouping full documents with jq. It does not yet establish the best production cache or the cost of maintaining it from third-party file changes.

In a separate DuckDB 1.5.6 test, `CREATE TABLE records AS SELECT id, path, data FROM read_ndjson(...)` inferred native nested types from the Musenalm records. `data.year` became `BIGINT`; the resulting database was about 24.4 MB, and grouping by that field took about 0.001 s. Import took 0.629 s from the already-built NDJSON files. This inferred structure from sample data, **not** from the repository's JSON Schema; it would need a policy for rare fields, mixed types, and later schema changes.

## Rust SQLite JSONB lifecycle benchmark

A separate, reproducible Rust benchmark now uses `rusqlite` 0.41.0 with bundled SQLite 3.54.1. It builds SQLite directly from the authoritative JSON source files, with no NDJSON staging step. The `records` table contains only `id` and JSONB `data`; a separate `sources` table stores source path, byte length, and modification time for change detection. It uses WAL, `synchronous=NORMAL`, a 256 MiB SQLite page-cache limit, and one transaction per build or batch. A full rebuild creates a new database file. Measurements below were on btrfs; the operating system's file cache was uncontrolled.

| Operation | 200,000 synthetic JSON files | Musenalm `entries` (4,928 larger JSON files) |
| --- | ---: | ---: |
| Fresh build, including WAL checkpoint | 3.113 s | 0.800 s |
| Full rebuild into a new database, including checkpoint | 2.820 s | 0.734 s |
| Scan unchanged source inventory | 0.928 s | 0.016 s |
| Scan after one changed source | 0.771 s | — |
| Refresh one changed record and commit | 0.005 s | 0.004 s |
| Checkpoint after that update | 0.009 s | 0.008 s |
| Scan after 1,000 changed sources | 0.715 s | — |
| Refresh 1,000 records in one transaction | 0.047 s | — |
| Refresh 100 records in one transaction | — | 0.008 s |
| Checkpoint after batch | 0.001 s | 0.015 s |
| Create a JSON-path expression index for `$.year` | 0.105 s | 0.019 s |
| Database size before / after year index and updates | 30.45 / 34.49 MB | 121.37 / 121.62 MB |

Source generation for the 200,000-file case took 7.841 s and is excluded from cache-build time. The Musenalm benchmark used temporary edited copies of records and did not modify the repository. SQLite parsed JSON into JSONB and rejected malformed JSON, but this benchmark did not perform JSON Schema validation. The source scan includes loading the database's source-stamp table and walking/statting all source files. A one-record update wrote an 8 KiB WAL; the 1,000-record batch wrote about 6.3 MB before checkpointing.

The initial synthetic build varied across local runs. The ordinary first-open time was about 0.02 s, but two runs paused for roughly 2.5–3.2 s while opening and configuring SQLite immediately after generating 200,000 files. Those cold totals reached 5.4–6.2 s. Warm full rebuilds were 2.5–2.8 s. This is why source generation and database opening are reported separately in the runner. The resulting cache is smaller than the earlier synthetic NDJSON plus index generation (about 52 MB), but it still must coexist with the authoritative source files.

## Embedded engine comparison: SQLite, Turso, SurrealDB

On 2026-10-27 I ran `src/bin/engine_compare.rs` in release mode with bundled SQLite 3.54.1 (`rusqlite` 0.41), Turso 0.8.3 (embedded Rust crate), and SurrealDB 3.3.4 with embedded SurrealKV. All three databases were temporary caches; the source JSON files remained authoritative and were not modified. The runner reads the source files into memory once, reports that read time separately, and then times database creation and ingestion from the same documents. A fresh build is also a full rebuild into a new database. The SQL engines store `records(id, data JSONB)`; SurrealDB stores records with the same `id` and nested `data` values. SQLite and Turso use WAL and `synchronous=NORMAL`. No schema validation, source-change scan, or cross-collection reference check is included in these figures.

Query times are averages of five warm executions including result retrieval. Filter means count records with year 1800–1899; sort means return the first 100 years in that range ordered by year and ID; group means count by year. Nested agent lookup counts records whose top-level `agents` array contains an object with a selected agent ID. Results of every comparable query had identical digests across engines. Measurements are single runs with uncontrolled OS caches and differing engine durability defaults, so they show workload behavior rather than stable speed ratios.

The first release build linking all three engines took 19 minutes 45 seconds and produced a 164 MB benchmark binary on this machine. That is a build/deployment cost of the combined experiment, not an estimate for shipping any one engine alone.

### 200,000 small synthetic JSON files

Source-file generation took 5–6 seconds and source reading took 0.88–0.91 seconds, both excluded from database ingestion below.

| Operation | SQLite | Turso | SurrealDB |
| --- | ---: | ---: | ---: |
| Fresh build and commit | 2.052 s | 0.790 s | 4.411 s |
| Filter | 0.0197 s | 0.1122 s | 0.1574 s |
| Sort first 100, no index | 0.0242 s | 0.1321 s | 0.0628 s |
| Group by year | 0.0510 s | 0.1299 s | 0.2472 s |
| Build `(year, id)` index | 0.0807 s | 0.3848 s | 11.273 s |
| Sort first 100, indexed | 0.000024 s | 0.000256 s | 0.00416 s |
| Update one cached record | 0.000041 s | 0.00471 s | 0.00487 s |
| Update 100 cached records in one transaction | 0.000468 s | 0.00166 s | 0.0101 s |
| Database storage after index and updates | 19.53 MB | 20.47 MB | 90.47 MB |

### Musenalm `entries` (4,928 larger nested JSON files)

Source reading took about 0.1 seconds. The sample agent ID was `lngx6eyfw5qrzas`; all three engines counted five matching entries before the simulated update.

| Operation | SQLite | Turso | SurrealDB |
| --- | ---: | ---: | ---: |
| Fresh build and commit | 0.747 s | 0.865 s | 6.320 s |
| Filter | 0.0235 s | 0.285 s | 1.082 s |
| Sort first 100, no index | 0.0232 s | 0.293 s | 0.389 s |
| Group by year | 0.143 s | 0.265 s | 0.854 s |
| Nested agent lookup, no index | 0.0307 s | 0.291 s | 0.983 s |
| Build `(year, id)` index | 0.0317 s | 0.298 s | 1.122 s |
| Sort first 100, indexed | 0.000031 s | 0.000146 s | 0.310 s |
| Update one cached record | 0.000070 s | 0.000273 s | 0.00410 s |
| Update 100 cached records in one transaction | 0.0190 s | 0.0308 s | 0.0859 s |
| Database storage after year index and updates | 121.41 MB | 123.14 MB | 194.76 MB |

SurrealDB's `EXPLAIN` output shows an index scan for the indexed year sort, despite its remaining 0.31-second latency on these large records. An additional ID-only projection through the same index still took about 0.29 seconds, so projecting `data.year` does not account for most of that latency. It can also index nested array values directly with `DEFINE INDEX records_agent ON records FIELDS data.agents.*.agent`. That index took about 1.0 second to build, raised storage to 195.56 MB, and reduced the agent count lookup to about **0.001 seconds** with the same answer. Retrieving the five matching record IDs through that index also took about **0.001 seconds**. After updating one cached record to remove that agent, the indexed count changed from five to four. SQLite and Turso used `json_each` for the equivalent unindexed lookup; this harness did not add a separate link table or text-search index to either engine.

The one-record and 100-record update timings are database-only: changed JSON values were constructed from the preloaded source documents. A real refresh must also discover external file changes, read changed files, validate them, and update the database. The earlier Rust SQLite lifecycle test above measures a 0.7–0.9-second unchanged-file scan over 200,000 files; that filesystem cost applies regardless of which embedded engine stores the cache. The SurrealDB index provides fast lookup of nested references but does not automatically enforce the project's nested `x-relation` rules.

## In-process array comparison: qj, SQLite, Node.js

Measured on 2026-10-28 on the same i5-1145G7 host, with qj 0.3.0, bundled SQLite 3.54.1 through rusqlite 0.41, and Node.js 26.8.1. The new `array-compare` runner builds one compact NDJSON input per collection from authoritative JSON files, then launches each engine in its own process. qj and Node.js parse the whole collection into an in-process array; SQLite ingests the same lines into a temporary JSONB table with `id TEXT PRIMARY KEY`. SQLite uses WAL, `synchronous=NORMAL`, and a 256 MiB page-cache limit. All comparable query results matched exactly.

Numbers below are medians of three separate process runs. Each query was warmed once, then measured seven times per process; its per-process median is the value aggregated here. The process runs were sequential, the OS file cache was uncontrolled, and the engines use their normal query APIs. Node.js queries are hand-written array operations; qj executes general jq expressions; SQLite executes SQL over JSONB. They deliver the same answers through different APIs and result representations. The figures test these implementations of the workload, not an equal amount of work at each instruction level.

| Operation | Musenalm `entries`: qj | SQLite | Node.js | 200k synthetic: qj | SQLite | Node.js |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Build from NDJSON | 0.338 s | 0.649 s | 0.615 s | 0.143 s | 0.458 s | 0.235 s |
| Rebuild from NDJSON | 0.313 s | 0.595 s | 0.579 s | 0.121 s | 0.414 s | 0.171 s |
| Count years 1800–1899 | 2.22 ms | 19.37 ms | **0.065 ms** | 63.28 ms | 29.61 ms | **3.08 ms** |
| Sort matching records, first 100 | 6.37 ms | 21.12 ms | **0.82 ms** | 122.08 ms | 36.09 ms | **25.76 ms** |
| Group numeric years | 5.17 ms | 70.07 ms | **0.31 ms** | 295.86 ms | 93.68 ms | **7.51 ms** |
| Lookup one ID | 0.798 ms | **0.004 ms** | 0.035 ms | 48.00 ms | **0.005 ms** | 1.56 ms |
| Nested agent count, no index | 5.74 ms | 21.60 ms | **1.26 ms** | — | — | — |
| Retrieve matching agent IDs, no index | 5.83 ms | 22.50 ms | **0.54 ms** | — | — | — |
| Build year index or saved sorted view | 6.70 ms | 31.36 ms | **1.08 ms** | 130.78 ms | 111.68 ms | **24.78 ms** |
| First 100 from index or saved view | 0.050 ms | 0.031 ms | **0.007 ms** | 0.078 ms | 0.032 ms | **0.010 ms** |

Musenalm has 4,928 relatively large nested records. Its shared NDJSON file was 127.13 MiB; preparing it from the original files took a median 1.081 s. The year count was 3,818, the group query returned 166 years, and the nested agent lookup found five entries. The synthetic dataset had 200,000 small records in a 14.71 MiB NDJSON file. Generating its 200,000 source files took 6.65–7.61 s, and preparing NDJSON from them took a median 1.338 s. Its year count was 40,000 and grouping returned 500 years. All source preparation and generation times are **excluded** from the engine build figures.

| Resource or update | Musenalm: qj | SQLite | Node.js | 200k: qj | SQLite | Node.js |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Process RSS after build | 426 MiB | **142 MiB** | 518 MiB | 105 MiB | **24 MiB** | 131 MiB |
| Peak process RSS in whole run | 738 MiB | **254 MiB** | 650 MiB | 137 MiB | **37 MiB** | 217 MiB |
| Update one | 0.063 ms | 0.058 ms | 0.034 ms | 0.094 ms | 0.052 ms | 0.049 ms |
| Update 100 | 0.194 ms | 4.43 ms | 0.023 ms | 0.208 ms | 24.90 ms | 0.028 ms |
| SQLite database size after updates | — | 115.78 MiB | — | — | 18.59 MiB | — |
| Reopen existing SQLite database | — | 0.466 ms | — | — | 0.286 ms | — |

The update rows have different persistence costs: qj and Node.js change only their in-memory arrays at known positions, while SQLite commits updates to its temporary database. The year “index” is also different: qj and Node.js materialize one specific sorted result, while SQLite's expression index supports other matching SQL queries and is maintained on updates. SQLite's query plan was checked to use that index. The cached SQLite reopen was measured with a warm OS cache and excludes the source-file reconciliation scan and first query; in-memory arrays must be reparsed after a process restart. Process RSS excludes filesystem page cache, and Node's baseline runtime contributes to its RSS.

On this workload, hand-written JavaScript over a loaded array is fastest for scans and grouping, while qj is the fastest to parse the Musenalm NDJSON into its array. SQLite is dramatically better for indexed ID lookup, process memory, and reuse of an existing cache at startup. At 200,000 records, SQLite also beats qj for unindexed sorting and grouping. Node.js's speed here comes from query-specific code; supporting arbitrary saved or user-defined views still needs a query interface and a plan for maintaining any materialized views. None of these measurements includes schema validation or detecting third-party edits in the authoritative JSON files.

## Expanded Musenalm suite and in-memory SQLite

The benchmark now compares **four** engines: qj, disk SQLite JSONB, SQLite JSONB in `:memory:`, and Node.js arrays. This section supersedes the three-engine Musenalm run above. Musenalm's registered `entries`, `agents`, `series`, and `places` collections supplied 4,928, 4,010, 1,202, and 210 records respectively. Each engine first built the entries collection, then loaded the related collections and built its normal ID lookup structures. Every comparable result matched across all four engines, including complete result arrays for the complex queries. Times are medians of three independent process runs; within each run, each query had one warm-up followed by seven timed executions. The operating system's file cache was uncontrolled.

The real Musenalm workloads were:

- Title page: sort entries by `preferred_title` and ID; return 50 rows after offset 100.
- Review facets: count entries by `edit_state` (five groups).
- Combined filter: years 1800–1850, `Edited`, language `ger`, at least one item, and a nested `Herausgeber:in` agent; count 1,065 matches and page 30 rows after offset 100.
- Title search: case-insensitive ASCII substring `almanach`; count all 1,315 matches separately from returning the first 50 IDs.
- Nested aggregation: count each `contents[].musenalm_type[]` value for edited entries from 1800–1899 (24 groups), and count `agents[].type` by decade (19 groups).
- Relations: top 20 agents and places by distinct linked entries, joining their names from the respective collections; top 20 series by matching entry count with year span, joining `series.entries[]` to entries by ID.

| Musenalm operation | qj array | SQLite disk | SQLite `:memory:` | Node.js array |
| --- | ---: | ---: | ---: | ---: |
| Build entries from NDJSON | **0.324 s** | 0.656 s | 0.340 s | 0.728 s |
| Load agents, series, places and ID maps/tables | 0.064 s | 0.129 s | **0.055 s** | 0.164 s |
| Title page, unindexed | 5.64 ms | 25.07 ms | 12.70 ms | **3.45 ms** |
| Review-status facets, unindexed | 4.10 ms | 78.37 ms | 66.68 ms | **0.27 ms** |
| Combined filter count | 8.40 ms | 19.06 ms | 12.13 ms | **0.67 ms** |
| Combined filter, page 30 | 9.51 ms | 2.45 ms | 1.53 ms | **0.73 ms** |
| Title search, count all | 3.82 ms | 23.12 ms | 12.63 ms | **0.65 ms** |
| Title search, first 50 IDs | 4.05 ms | 0.72 ms | **0.40 ms** | 0.74 ms |
| Nested content-type aggregation | 134.30 ms | 260.34 ms | 242.75 ms | **9.99 ms** |
| Agent role by decade | 9.04 ms | 94.87 ms | 89.09 ms | **2.28 ms** |
| Top agents joined to names | 13.10 ms | 59.72 ms | 48.18 ms | **2.83 ms** |
| Top places joined to names | 7.25 ms | 25.07 ms | 14.38 ms | **1.18 ms** |
| Series entries joined to year spans | 5.94 ms | 31.13 ms | 19.91 ms | **0.81 ms** |

For the actual list view, SQLite then created expression indexes for title ordering and review status. Building both took **25.4 ms on disk** and **25.9 ms in memory**. The query plan was checked to use each index. With those indexes, the title page took **0.143 ms on disk / 0.095 ms in memory**, and review facets took **0.245 / 0.235 ms**. These are reusable SQLite indexes maintained on updates; the Node and qj results above did not build corresponding title or status indexes. A first-page title search can stop after finding 50 IDs; its full count is the appropriate full-scan comparison.

Preparing entries NDJSON from the authoritative files took a median **1.079 s**; preparing agents, series, and places took another **0.160 s**. These times are excluded from every engine build figure. The disk SQLite cache occupied **135.98 MiB** after all indexes and updates. Process RSS after loading the related collections was about **504 MiB qj**, **164 MiB disk SQLite**, **164 MiB in-memory SQLite**, and **593 MiB Node.js**. Peak RSS across the entire run was about 756, 321, 321, and 747 MiB respectively. Disk SQLite also uses OS page cache that is not included in process RSS.

On 200,000 small synthetic records, adding in-memory SQLite gave these medians:

| Operation | qj | SQLite disk | SQLite `:memory:` | Node.js |
| --- | ---: | ---: | ---: | ---: |
| Build from NDJSON | 0.141 s | 2.243 s* | 0.301 s | 0.184 s |
| Rebuild from NDJSON | **0.105 s** | 0.390 s | 0.290 s | 0.144 s |
| Count years 1800–1899 | 58.45 ms | 26.32 ms | 25.99 ms | **2.67 ms** |
| Sort matching records, first 100 | 107.73 ms | 32.11 ms | 31.63 ms | **21.17 ms** |
| Group by year | 245.43 ms | 88.22 ms | 86.03 ms | **7.09 ms** |
| ID lookup (indexed in SQLite) | 38.21 ms | 0.005 ms | **0.004 ms** | 1.36 ms |
| Build year index or saved sorted view | 109.00 ms | 103.90 ms | 79.81 ms | **21.08 ms** |

*Disk SQLite's first build varied from **0.410 to 2.328 s** across these runs; its full rebuild was a steadier **0.379–0.394 s**. Each synthetic run generated 200,000 files immediately before the database build, so delayed filesystem work may have affected the first build. Do not treat the 2.243-second median as a stable SQLite import cost. The in-memory build avoids disk writes but must be repeated at each process start. Disk SQLite reopened an existing cache in about **0.26 ms** with a warm OS cache, excluding its first query and any source-change scan.

The comparison still has distinct execution models. Node's functions are tailored JavaScript over live objects. qj and SQLite run general query languages; qj and Node keep all source values in memory. SQL uses `json_each` for nested arrays, without specialized nested indexes or link tables in this suite. SQLite's year, title, and status indexes help only queries that match their expressions. The array update figures are in-memory mutations; disk SQLite updates are committed. No benchmark here includes validating schemas, reconciling external JSON-file changes, or measuring cold startup after an OS cache drop.

## Node.js reading authoritative files directly

The previous Node.js baseline read an entire NDJSON cache into one UTF-8 string, split it into lines, then parsed the array. To test whether the cache is useful at all for Node, the runner now also measures four other loaders:

- **Chunked NDJSON:** read the cache in 1 MiB chunks and parse one JSON line at a time.
- **Original files, synchronous:** recursively discover JSON files and call `readFileSync` plus `JSON.parse` for each; derive IDs from filenames.
- **Original files, async:** the same files, with up to 32 outstanding `fs.promises.readFile` requests, using Node's default I/O thread pool.
- **Original files, async16:** the same bounded requests with `UV_THREADPOOL_SIZE=16`.

All five Node loaders produce the same in-memory array shape and matched the qj and SQLite query results, including Musenalm's cross-collection joins. Values below are medians of three independent process runs on 2026-10-28. As in the other runs, the OS file cache was uncontrolled. The benchmark prepares NDJSON before launching the direct-file variants, so their original-file reads can benefit from a warmed OS cache. Node process startup is excluded from its `build` timer.

| Musenalm (4,928 entries plus agents, series, places) | Whole NDJSON | Chunked NDJSON | Original files sync | Original files async | Original files async16 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Load entries | 0.951 s | 0.690 s | **0.682 s** | 0.715 s | 0.776 s |
| Load related collections and ID maps | 0.200 s | **0.139 s** | 0.189 s | 0.200 s | 0.211 s |
| Total entries plus related load | 1.151 s | **0.828 s** | 0.884 s | 0.908 s | 0.998 s |
| Reload entries | 0.911 s | **0.574 s** | 0.646 s | 0.673 s | 0.668 s |
| Process RSS after all collections | 593 MiB | **329 MiB** | 341 MiB | 364 MiB | 368 MiB |
| Peak RSS during full run, including reload | 737 MiB | **493 MiB** | 493 MiB | 510 MiB | 518 MiB |

Preparing the entries NDJSON took another **1.099 s**, and preparing the three related NDJSON files took **0.165 s**. Thus a first-time cache build plus chunked Node load cost about **2.09 s** in this setup, versus **0.88 s** for direct synchronous loading from already warm original files. Once the NDJSON cache exists, chunked loading was slightly faster than reopening all original files. Direct loading has the additional advantage that each startup sees the current source files without a cache reconciliation step.

| 200,000 small original JSON files | Whole NDJSON | Chunked NDJSON | Original files sync | Original files async | Original files async16 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Load records | **0.201 s** | 0.265 s | 1.436 s | 1.780 s | 1.898 s |
| Reload records | **0.142 s** | 0.210 s | 1.258 s | 1.884 s | 2.152 s |
| Process RSS after load | 133 MiB | **130 MiB** | 267 MiB | 272 MiB | 273 MiB |
| Peak RSS during full run, including reload | 222 MiB | **197 MiB** | 321 MiB | 353 MiB | 357 MiB |

Preparing this synthetic NDJSON from the generated source files took **1.244 s**. One-time preparation plus whole-file Node load was therefore about **1.44 s**, close to the **1.44 s** direct synchronous load in this warmed-cache experiment. On later starts, the NDJSON load was about seven times faster. Source-file generation itself took roughly five seconds and is excluded from both paths.

The earlier ~593 MiB Musenalm Node RSS was largely a **whole-file parser peak**, not the memory inherently required for the live objects: chunked NDJSON and direct-file loading used roughly 329–341 MiB after loading all four collections. All variants still keep the parsed collection in memory while querying. Async per-file I/O did not improve this local workload; more concurrent reads increased memory and were slower at 200,000 files. A persistent NDJSON cache helps most when file count is very high or the same collection is reloaded repeatedly. For Musenalm's current file count, direct original-file loading is a credible simpler option, especially because it avoids a separate cache that must track outside edits. Cold-disk behavior remains unmeasured.

## Full Musenalm Node.js data load, including standalone contents

The preceding Musenalm suite omitted `data/contents`, `data/items`, and `data/pages`. A separate [Node runner](src/bin/musenalm_node_full.mjs) now loads **all seven `data` directories**, including unregistered collections. This covers **148,486 JSON files**: 133,948 contents, 4,928 entries, 4,010 agents, 4,180 items, 1,202 series, 210 places, and 8 pages. Source JSON totals **341.9 MiB apparent bytes** but occupies about **765.3 MiB of allocated file blocks** on this filesystem. The generated compact NDJSON files total **269.2 MiB**. The `entries` objects contain 133,948 nested content records with `source_id` links; every one resolved to a standalone content file in this dataset. Loading all seven directories therefore retains both representations.

The Node runner enumerates and parses each source file synchronously, tries up to 32 concurrent async reads, or parses existing NDJSON in 1 MiB chunks. It builds ID maps after loading. All modes run the same hand-written JavaScript queries over the parsed in-memory objects. Values are medians of **three fresh Node processes** per mode on 2026-10-28; each query has one warm-up and seven timed runs per process. Query-result digests matched across all modes and runs, including the saved title view. Runs were interleaved, but the OS page cache was warm and not controlled. The Node process was launched with `--expose-gc --max-old-space-size=8192`; the load timer excludes process startup.

| Full data operation | Original files, sync | Original files, 32 async reads | Existing chunked NDJSON |
| --- | ---: | ---: | ---: |
| Load all seven directories | **2.501 s** | 2.699 s | **1.477 s** |
| Of which: load `contents` | 1.480 s | 1.681 s | **0.653 s** |
| Build ID maps after load | 0.089 s | 0.096 s | 0.086 s |
| RSS after load and explicit GC | 575 MiB | 603 MiB | 493 MiB |
| RSS after ID maps | 595 MiB | 624 MiB | 511 MiB |
| Peak RSS through all queries | 611 MiB | 645 MiB | 538 MiB |

| Query over the loaded data | Original files, sync | Existing NDJSON | Result |
| --- | ---: | ---: | --- |
| Count contents from 1800–1899 | 4.7 ms | 4.9 ms | 95,153 |
| Filter edited contents, sort by title/ID, return rows 100–149 | 180.5 ms | 166.5 ms | 50 rows |
| Content type facets in that century | 18.6 ms | 17.9 ms | 24 groups |
| Search content titles for `almanach` | 27.2 ms | 21.9 ms | 909 matches |
| Search titles and annotations for `almanach` | 47.9 ms | 42.6 ms | 1,919 matches |
| Resolve all entry-to-content links via ID map | 29.4 ms | 31.8 ms | 133,948 / 133,948 resolved |
| Resolve all entry-to-item links via ID map | 1.2 ms | 1.1 ms | 4,180 / 4,180 resolved |
| Top content contributors joined to agent names | 35.3 ms | 36.4 ms | 20 rows |

The full title page sorts about 95,000 matching records every call. Materializing that **specific** filtered title order once took **0.171 s** from direct files or **0.165 s** from NDJSON; returning its 50-row page then took roughly **0.005–0.006 ms** in this microbenchmark. This is a saved view, not a general index. It must be refreshed when a relevant source record changes. A content ID lookup using the map was also below 0.001 ms; that timing is too small to interpret as an end-to-end UI latency.

Building the seven NDJSON files from original JSON took **3.37 s** (median of three runs). Walking and `stat`ing all 148,486 source files without parsing them took **0.596 s** (median of three runs), most of it in `contents`. This inventory scan is only a lower bound for cache freshness checking: it does not compare a saved manifest or catch changes that preserve size and modification time. Thus first build plus cached load was about **4.84 s**, versus **2.50 s** for a direct full load. On a later warm start, cached load plus this scan was about **2.07 s**, only about **0.43 s** below direct loading. Skipping freshness checks makes the cache faster, but can present stale data. Cache building did not include an `fsync`, schema validation, or atomic publication of all seven files.

These results correct the scope of the earlier conclusion. The real Musenalm repository has a high file count; the earlier four-collection run did not show that direct loading would remain fastest across all its data. Yet full direct loading was still practical in this warmed-cache experiment, and the cache's advantage narrowed once a basic source scan was included. The 200,000-file synthetic case remains useful for scaling, but its smaller documents and different filesystem behavior cannot decide the Musenalm architecture alone. No full-data SQLite, qj, or jaq comparison was run here, and the search query checks ordinary strings rather than normalized Markdown, HTML, or XML.

## Full Musenalm direct-file Rust and Node.js comparison

To test whether a future Rust UI would inherently be slower than Node, we ran the same content queries on all seven source directories through two Rust and two Node representations. The [generic Rust runner](src/bin/musenalm_rust_full.rs) retains every JSON field as `serde_json::Value`; the [projected Rust runner](src/bin/musenalm_rust_projected.rs) deserializes directly into typed view fields. The Node runner has corresponding full and projected modes. Both projected modes retain content year, state, title, annotation, types and contributor IDs, entry-to-content/item IDs, and agent names; they discard other fields after parsing. **All modes still open and parse all 148,486 source files.** The projected Rust parser can skip unwanted JSON fields during deserialization, while Node first parses the whole object and then projects it. These are realistic implementation options, not identical parser instructions.

The numbers are medians of three separate release Rust or Node processes, each with one warm-up and seven timed query runs. The OS page cache was warm and uncontrolled. All comparable query-result arrays and counts matched by digest, including the saved title page. Memory is process RSS after loading, before any explicit Node GC in the full loader; the full Node loader dropped to about 575 MiB after explicit GC.

| Representation | Load all files | RSS after load | Build ID maps | Filter/sort 95k contents for title page | Content type facets |
| --- | ---: | ---: | ---: | ---: | ---: |
| Node, full objects | 2.501 s | 579 MiB | 89 ms | 180.5 ms | 18.6 ms |
| Rust, generic `serde_json::Value` | 2.965 s | 1,520 MiB | 19 ms | 425.9 ms | 73.9 ms |
| Node, projected objects | 2.524 s | 304 MiB | 76 ms | 128.1 ms | 10.2 ms |
| Rust, typed projection | **1.246 s** | **86 MiB** | **14 ms** | **54.3 ms** | **8.6 ms** |

| Query on projected fields | Node | Rust | Matching result |
| --- | ---: | ---: | --- |
| Count 1800–1899 contents | 3.9 ms | **1.4 ms** | 95,153 |
| Search content titles for `almanach` | **17.3 ms** | 21.1 ms | 909 |
| Search title and annotation for `almanach` | **38.5 ms** | 63.5 ms | 1,919 |
| Resolve all entry-to-content links | **11.5 ms** | 24.4 ms | 133,948 resolved |
| Resolve all entry-to-item links | 0.4 ms | **0.2 ms** | 4,180 resolved |
| Top content contributors joined to agent names | **24.0 ms** | 28.6 ms | 20 rows |
| Build the filtered title order once | 143.2 ms | **54.0 ms** | 95,153 rows |

This rejects the inference that Node query speed requires an Electron UI. The generic Rust value tree was relatively costly, but typed Rust projections on the same source files were faster and much smaller for several measured operations. Node remained faster for some string and relation loops. None of these results measures UI rendering, truly lazy per-view loading, schema validation, or the revised MVP's dynamic view model. A typed projection only supports its selected fields. **It is not an implementation of the MVP's generic query API**, which accepts a PocketBase-inspired filter and view specification over any configured collection. Persistent caching is optional, and the next deciding benchmark must exercise the same API through a collection-neutral runner.
