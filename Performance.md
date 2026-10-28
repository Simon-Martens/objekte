# Performance findings

The [disk-cache startup follow-up](crates/bench/CLI-LATENCY-2026-10-28.md) measures
the current CLI using a copy of the application cache on Btrfs. Unchanged `contents`
opens fell from **0.531 s to 0.374 s release**, and **1.706 s to 1.466 s debug**;
`entries` fell from 90 ms to 32 ms release. Avoiding unchanged database writes,
directory-relative metadata lookups on Linux, and buffered manifest hashing preserve
existing cache fingerprints. The remaining `contents` delay is dominated by its
133,948-file metadata inventory. Use `cargo run --release` when evaluating latency.

The [metadata-manifest update](crates/bench/METADATA-MANIFEST-2026-10-28.md)
previously reduced unchanged Musenalm `contents` CLI opens to **0.463 s release** (median of
three processes), with one metadata inventory and zero source reads. Strict
full-content checking remains available and takes 2.355 s. Separate compound view
fingerprints and secondary sorts are now implemented; cache format 2 rebuilds
older databases once.

The [earlier full-hash startup profile](crates/bench/LIBRARY-PROFILE-2026-10-28.md)
measured about 3.5 seconds to verify an unchanged cache in release mode and about
1.4 ms for subsequent default views on the same repository handle. It established
that source filesystem checks dominated startup while the title index was used
correctly. These figures describe the implementation before the manifest update.

A [fresh rerun on 28 October 2026](crates/bench/RERUN-2026-10-28.md) records direct Rust, Node full objects, Rust SQLite JSONB disk cache, and embedded qj on a different CPU. The historical measurements below remain separate.

## 28 October shared-workload correction

The new [shared runner](crates/bench/src/bin/shared_query_compare.rs) used the **same records and checked complete result equality** for qj, jaq, jq, SQLite JSONB, Node, and native Rust. Field names are runtime arguments. It ran on Musenalm `contents` (133,948 files, 126.7 MB compact NDJSON) and Lenz `sekundaerliteratur` (2,133 files). The four operations were integer range count, range filter plus numeric/ID sort and first 50 rows, grouping by integer field, and ASCII case-insensitive text count. Musenalm results were 95,153 matches, 50 rows, 122 groups, and 909 text matches. This is a shared **scalar** workload, not the MVP's full nested/array/relation query contract.

Musenalm figures are medians of **three fresh processes**, each with one warm-up and three timed queries. Loads include parsing/import but exclude process startup. Direct-file variants enumerate and read original JSON. qj and jaq load an already-built NDJSON file. SQLite memory imports from original files. The OS file cache was warm and uncontrolled.

| Engine and representation | Load | RSS after load | Range count | Sort/page 50 | Group | Text count |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Rust runtime field projection, direct files | 1.365 s | 26 MiB | **0.49 ms** | **13.64 ms** | **4.73 ms** | **8.07 ms** |
| Node runtime field projection, direct files | 1.467 s | 192 MiB | 3.60 ms | 53.79 ms | 5.51 ms | 16.93 ms |
| Node full objects, direct files | 1.507 s | 281 MiB | 5.65 ms | 61.74 ms | 7.39 ms | 22.53 ms |
| Rust full `serde_json::Value`, NDJSON | 0.982 s | 823 MiB | 63.06 ms | 82.75 ms | 74.82 ms | 69.30 ms |
| SQLite JSONB `:memory:`, direct files, no index | 1.139 s | 141 MiB | 118.13 ms | 149.23 ms | 311.81 ms | 92.64 ms |
| Embedded qj, NDJSON array | 0.566 s | 417 MiB | 127.80 ms | 335.34 ms | 293.52 ms | 198.38 ms |
| Embedded jaq, NDJSON array | 1.793 s | 1,004 MiB | 886.24 ms | 1,336.44 ms | 914.25 ms | 476.37 ms |

Node chunked-NDJSON load was 0.728 s and used 273 MiB RSS in one process; its warm queries took 5.7, 45.0, 7.5, and 21.7 ms. SQLite disk JSONB import directly from source took 1.679 s in one process; unindexed queries resembled SQLite memory. Rust full-value **direct-file** load was 2.102 s. The Rust NDJSON sort was corrected from an initial 775 ms to about 83 ms by extracting keys once before sorting. That old result was an avoidable comparator cost. Both projected runners parse every JSON document, choose field names at runtime, and retain only ID plus the requested integer and text fields. Their four operations are hand-written, so they are encouraging lower bounds rather than completed general query engines.

Preparing NDJSON from the original contents files took **1.623 s**. Thus first preparation plus qj load was about **2.19 s**, or plus jaq load about **3.42 s**, before any query. A persisted SQLite database opened in about **1 ms**. Creating an expression index on `year, id` took **0.293 s**. After reopening the indexed database, medians across three processes were **14.51 ms** range count, **0.04 ms** first page, **11.48 ms** grouping, and **95.79 ms** unindexed text count. `EXPLAIN QUERY PLAN` confirmed a covering index search for the page. The disk database occupied about **128 MB**. Walking and `stat`ing all 133,948 source files took **0.232–0.237 s** across three runs, before comparing saved stamps; that reconciliation cost belongs in cached-startup latency. Lenz direct loads in one process were 36 ms projected Rust, 43 ms Node, and 17 ms SQLite memory. All compared Lenz outputs agreed.

For a first large view, approximate load plus warm page-query times were 1.38 s direct projected Rust, 1.52 s direct projected Node, and 1.29 s direct SQLite memory. Those are combinations of separate measured phases, not a cold-first-query measurement. The existing indexed disk SQLite path is roughly 0.23 s for a basic source scan plus open and warm page query, excluding stamp comparisons and any needed updates. First disk build from original files plus the year index was about 1.97 s. An in-memory SQLite database pays its import cost on every application start.

`jq 1.8.1` was tested as a **fresh subprocess per query**, slurping the existing NDJSON each time. Musenalm medians over three invocations were 2.66 s count, 3.26 s page, 3.31 s group, and 6.13 s text, with roughly 900 MiB peak RSS. These include process startup and reparsing, so they are **not warm in-process equivalents** of qj/jaq. jq matched all four outputs. On Lenz the same fresh-process queries took 26–271 ms.

**Current recommendation:** retain authoritative JSON and implement the library/CLI in Rust. Open a repository cheaply; load only collections needed by a view. Compile the MVP's bounded filter/view specification into a plan that extracts needed fields and relation IDs at runtime, sorts compact keys, and keeps source identity for editing. Reuse active projections and default/saved view results in a bounded in-process key-value cache; admit repeated ad hoc views by measured cost and frequency. Make neither NDJSON, an in-memory database, nor a separate disk KV/result cache mandatory. A KV lookup helps known-ID reads, but a new arbitrary filter or sort still needs a scan or secondary index; cached view results need invalidation after outside edits. Use SQLite JSONB as an optional, disposable **indexed accelerator** for measured hot views. Index observed hot fields/views. [SQLite documents](https://www.sqlite.org/json1.html#the_jsonb_format) that JSONB does not provide O(1) field lookup, and [expression indexes](https://www.sqlite.org/expridx.html) require matching query expressions. This architecture allows a later native Rust UI.

The next decision gate is a compiled **general** filter/view executor with nested paths, arrays, relations, pagination, and source-change handling, benchmarked through the same public library API on Musenalm, Lenz, and generated data. The 13.64 ms projected Rust sort is evidence that Rust can be fast, not a promise for all dynamic views.

I also reran the older **Musenalm entries plus agents/series/places** complex suite with three warm query executions in one process per engine. All comparable outputs matched. The 50-row title page took 9.5 ms qj, 28.8 ms disk SQLite, 17.1 ms memory SQLite, and 3.4 ms Node direct files; an indexed disk SQLite title page took 0.19 ms. The top-agent join took 21.0, 74.4, 59.0, and 3.7 ms respectively. This older suite uses custom query implementations per engine and covers 4,928 `entries`, not the 133,948 standalone `contents`; its figures should not be mixed into the table above as a speedup ratio.

This is the performance record for the Objekte rebuild discussions and experiments of 27–28 October 2026. The individual measurements and runnable commands are in [the benchmark results](crates/bench/RESULTS.md) and [benchmark README](crates/bench/README.md). This document connects those experiments to the architecture decision. The revised [MVP](MVP.md) uses a small PocketBase-inspired query model and requires a **collection-neutral benchmark**. The qj/jaq and hand-written query results below are exploratory measurements, not a performance decision for that model.

## What we are optimizing

Individual JSON files in repository folders remain authoritative. The proposed application keeps JSON Schema for structure and validation, UI options for presentation, and view definitions for filtering, sorting, paging, grouping, and declared relation traversal. The library and CLI come before a new local UI. The required access modes are structural views, raw grep over source files, and search over cleaned text. The experiments below mostly measure **structural access**; the two text-search modes have not been benchmarked.

The meaningful workload is not one query in isolation. We need to account for source discovery, loading or building a disposable cache, opening the application again, query latency, memory, updates, and detecting edits made by other programs. “Fastest” depends on which of these costs is included.

The main datasets were:

| Dataset | What was measured |
| --- | --- |
| Musenalm `entries` | 4,928 large, nested JSON files; about 180 MB of source JSON and 127 MiB of compact NDJSON in the latest runner. |
| Musenalm related collections | 4,010 `agents`, 1,202 `series`, and 210 `places`, loaded for the first cross-collection views. That suite did **not** load every JSON file in the repository. |
| Full Musenalm `data` tree | 148,486 JSON files across seven directories, including 133,948 standalone `contents` and 4,180 `items`. Measured separately with Node.js in section 7. |
| Synthetic | 200,000 small JSON source files, used to expose file-count and index costs. Its latest compact NDJSON was about 14.7 MiB. |
| Lenz `sekundaerliteratur` | 2,133 records in the first NDJSON cache experiment. |

Runs used an Intel Core i5-1145G7 Linux machine (4 cores, 8 threads), release builds, and either btrfs under `target/` or tmpfs under `/tmp`. The OS file cache was not controlled. Later array and Node-loading figures are medians of three separate process runs, with one warm-up and seven timed executions per query. Earlier engine comparisons are single runs or use a different harness. **Do not divide numbers from different tables to claim a precise speedup.**

## 1. Sharded NDJSON cache and embedded qj/jaq

The Rust library's first cache uses 64 deterministic NDJSON shards per collection plus ID indexes. Files remain authoritative; a cache generation is rebuildable and a known-ID refresh rewrites one shard. This tested the idea of putting IDs at predictable positions while retaining a map to source path, shard, offset, and row length. A variable-length record still cannot be replaced safely in place without shifting following bytes.

| Operation, disk-backed | 200k synthetic | Musenalm entries | Lenz secondary literature |
| --- | ---: | ---: | ---: |
| Cold cache build | 2.819 s | 2.054 s | 0.034 s |
| Full rebuild | 2.159 s | 2.358 s | 0.027 s |
| Unchanged source inventory scan | 1.093 s | 0.017 s | 0.006 s |
| Refresh one known record's shard | 0.009 s | 0.039 s | 0.002 s |
| Open published generation | <0.001 s | <0.001 s | <0.001 s |
| Load full ID index | 0.285 s | 0.008 s | 0.002 s |

An initial single 29 MB JSON index for 200,000 IDs made targeted refresh take roughly 0.2–0.34 s. Splitting that index into 64 parts reduced it to 0.009 s. The synthetic NDJSON plus index occupied about 52 MB. A same-size external edit that preserves its modification time can evade the current stamp-based change detector. On tmpfs the 200k cold build was 1.994 s and the unchanged scan 0.871 s, showing how much filesystem choice matters.

The benchmark embeds qj 0.3.0 and jaq-all 0.3.0 as Rust crates, rather than invoking commands. Four workers read the same shards for independent-record filters:

| Query | 200k qj / jaq | Musenalm qj / jaq |
| --- | ---: | ---: |
| Project every ID | 0.087 / 0.252 s | 0.246 / 1.384 s |
| Guarded year filter and ID | 0.090 / 0.605 s | 0.317 / 1.647 s |

The engines agreed on output counts in those filters. This first filter test counted outputs without comparing or serializing every output value. qj's specialized tape path was **not** exercised. Whole-collection expressions loaded one array and used one thread; their result values were compared:

| Whole-collection query, including array parsing and output serialization | 200k qj / jaq | Musenalm qj / jaq |
| --- | ---: | ---: |
| Sort, first 100 | 0.780 / 1.385 s | 0.913 / 3.048 s |
| Group by year | 0.558 / 1.047 s | 0.646 / 2.598 s |
| Filter, then sort first 100 | 0.488 / 1.660 s | 0.604 / 2.433 s |

The key distinction is that per-record filtering parallelizes over shards, while global sorting and grouping require a collection-wide operation and memory proportional to the collection in this implementation. qj was faster than jaq in these tested embedded paths. This did not establish the fastest possible qj configuration or prove that jq-compatible programs alone solve joins and paging efficiently.

## 2. Exploratory DuckDB and SQLite JSON queries

An early Python-binding experiment put only `id` and untyped JSON `data` in DuckDB 1.5.6 and SQLite 3.53.1. The input was an already-built NDJSON cache; times were medians of five warm query executions, including Python result retrieval. This was **not** a fair Rust ingestion comparison.

| Query | 200k DuckDB JSON | 200k SQLite JSONB | Musenalm DuckDB JSON | Musenalm SQLite JSONB |
| --- | ---: | ---: | ---: | ---: |
| Filter IDs | 0.037 s | 0.136 s | 0.094 s | 0.037 s |
| Sort first 100 | 0.017 s | 0.056 s | 0.121 s | 0.035 s |
| Group by year | 0.029 s | 0.124 s | 0.173 s | 0.215 s |

SQLite text JSON was also tried; it was sometimes faster and sometimes slower than JSONB for these expressions. An expression index on `json_extract(data,'$.year'), id` reduced matching SQLite sorts to under 1 ms. Building it took 0.161 s on 200k records and 0.044 s on Musenalm in this Python experiment. The cost and usefulness depend on which fields actual views filter and sort.

DuckDB could infer native nested columns from NDJSON without reading the repository's JSON Schema. On Musenalm, `data.year` became `BIGINT`; that database was about 24.4 MB, import took 0.629 s from the existing NDJSON, and grouping by year took about 0.001 s. This is a promising typed analytical path, but type inference from sample data needs a rule for rare fields, mixed types, and later changes. A checkpoint after a single update temporarily doubled the observed Musenalm DuckDB file from about 136 MB to 272 MB in this experiment. We did not build a production DuckDB cache around source reconciliation.

## 3. SQLite cache lifecycle from original JSON files

A separate Rust benchmark used bundled SQLite 3.54.1 with `records(id, data JSONB)` and a `sources` stamp table. It read authoritative source files directly, used WAL, `synchronous=NORMAL`, a 256 MiB page cache, and batched transactions. There was no NDJSON staging step in these numbers.

| Operation | 200k small files | Musenalm entries |
| --- | ---: | ---: |
| Fresh database build including checkpoint | 3.113 s | 0.800 s |
| Full rebuild into a new file | 2.820 s | 0.734 s |
| Scan unchanged source inventory | 0.928 s | 0.016 s |
| Refresh one record and commit | 0.005 s | 0.004 s |
| Refresh a batch | 1,000 in 0.047 s | 100 in 0.008 s |
| Build year expression index | 0.105 s | 0.019 s |

The 200k cold build was variable: some runs paused for 2.5–3.2 s when opening/configuring SQLite immediately after generating the files, while warm full rebuilds were around 2.5–2.8 s. SQLite rejected malformed JSON during JSONB conversion, but this is not JSON Schema validation. The source scan compares paths, sizes, and modification times and is a separate cost from a one-record database update.

## 4. Embedded SQLite, Turso, and SurrealDB

One Rust runner compared bundled SQLite, Turso 0.8.3, and SurrealDB 3.3.4 with local SurrealKV. All were disposable caches of the same preloaded documents. The runner reported source-file reading separately, then measured ingestion and five warm queries. Results agreed by digest. These were single local runs with differing durability defaults.

| 200k synthetic | SQLite | Turso | SurrealDB |
| --- | ---: | ---: | ---: |
| Build and commit | 2.052 s | **0.790 s** | 4.411 s |
| Filter | **0.0197 s** | 0.1122 s | 0.1574 s |
| Sort first 100, no index | **0.0242 s** | 0.1321 s | 0.0628 s |
| Group by year | **0.0510 s** | 0.1299 s | 0.2472 s |
| Build year/ID index | **0.0807 s** | 0.3848 s | 11.273 s |
| Sort first 100, indexed | **0.000024 s** | 0.000256 s | 0.00416 s |

| Musenalm entries | SQLite | Turso | SurrealDB |
| --- | ---: | ---: | ---: |
| Build and commit | **0.747 s** | 0.865 s | 6.320 s |
| Filter | **0.0235 s** | 0.285 s | 1.082 s |
| Group by year | **0.143 s** | 0.265 s | 0.854 s |
| Nested agent lookup, no index | **0.0307 s** | 0.291 s | 0.983 s |
| Sort first 100, indexed | **0.000031 s** | 0.000146 s | 0.310 s |

SurrealDB could directly index `data.agents.*.agent`. That additional index took about 1.0 s to build and reduced the five-match agent lookup to about **0.001 s**. Its plan showed an index scan, and a cached-record update changed the indexed count from five to four. The SQL engines' nested lookups in this runner were **unindexed `json_each` scans**; there was no equivalent SQL link table or nested index, so the unindexed row must not be presented as a complete verdict on indexed relations. SurrealDB's index did not itself enforce the repository's nested relation rules. The combined three-engine release binary took 19 minutes 45 seconds to build and was 164 MB; that is a cost of this combined experiment, not a per-engine distribution estimate.

## 5. qj, SQLite disk and memory, and Node.js arrays

One phase of the later [array comparison](crates/bench/src/bin/array_compare.rs) prepared a shared compact NDJSON file, then ran engines in separate processes: embedded qj array, SQLite JSONB on disk, SQLite JSONB in `:memory:`, and Node.js parsed arrays. Node's view logic was hand-written JavaScript; qj and SQLite executed general query languages. Query results were checked for exact equality, including the complex Musenalm outputs. Three independent process runs with seven timed executions per query gave these medians. A subsequent phase added the original-file and chunked Node loaders in section 6, so its Node load timings come from different runs.

| Musenalm operation | qj | SQLite disk | SQLite `:memory:` | Node array from whole NDJSON |
| --- | ---: | ---: | ---: | ---: |
| Load entries from NDJSON | 0.324 s | 0.656 s | 0.340 s | 0.728 s |
| Load agents, series, places and ID maps/tables | 0.064 s | 0.129 s | 0.055 s | 0.164 s |
| Title page, no title index | 5.64 ms | 25.07 ms | 12.70 ms | **3.45 ms** |
| Review-status facets, no status index | 4.10 ms | 78.37 ms | 66.68 ms | **0.27 ms** |
| Combined nested filter count | 8.40 ms | 19.06 ms | 12.13 ms | **0.67 ms** |
| Nested content-type aggregation | 134.30 ms | 260.34 ms | 242.75 ms | **9.99 ms** |
| Top agents joined to names | 13.10 ms | 59.72 ms | 48.18 ms | **2.83 ms** |
| Top places joined to names | 7.25 ms | 25.07 ms | 14.38 ms | **1.18 ms** |
| Series linked to entry year spans | 5.94 ms | 31.13 ms | 19.91 ms | **0.81 ms** |

Other real queries included a 30-row page after a combined year/status/language/item/agent-role filter, title substring search as both full count and first page, and agent roles by decade. The combined filter had 1,065 matches; the title substring `almanach` had 1,315. The full query definitions and all timings are in [the detailed results](crates/bench/RESULTS.md#expanded-musenalm-suite-and-in-memory-sqlite).

SQLite then built title-order and review-status expression indexes in about **25 ms**. The indexed title page took **0.143 ms on disk / 0.095 ms in memory**; indexed review facets took **0.245 / 0.235 ms**. Plans were checked to use the indexes. These SQLite indexes are reusable and updated with the database; the Node and qj results above did not build corresponding title/status indexes. The year index likewise made the indexed first-100 query about 0.03 ms in SQLite. A first-page search can stop early, so its full count is the proper full-scan comparison.

| 200k small records | qj | SQLite disk | SQLite `:memory:` | Node whole NDJSON |
| --- | ---: | ---: | ---: | ---: |
| Load/build from NDJSON | 0.141 s | 2.243 s* | 0.301 s | 0.184 s |
| Rebuild from NDJSON | 0.105 s | 0.390 s | 0.290 s | 0.144 s |
| Count years 1800–1899 | 58.45 ms | 26.32 ms | 25.99 ms | **2.67 ms** |
| Sort first 100 matching | 107.73 ms | 32.11 ms | 31.63 ms | **21.17 ms** |
| Group by year | 245.43 ms | 88.22 ms | 86.03 ms | **7.09 ms** |
| ID lookup | 38.21 ms | 0.005 ms | **0.004 ms** | 1.36 ms |

*The disk SQLite first build ranged from 0.410 to 2.328 s immediately after generating 200,000 files. Its rebuild was a steadier 0.379–0.394 s. Treat the 2.243 s median as unstable. Disk SQLite could reopen an existing cache in about 0.3–0.5 ms with a warm OS cache, excluding source reconciliation and the first query. An in-memory database must be rebuilt on every process start.

With all four Musenalm collections loaded, process RSS was about **504 MiB qj**, **164 MiB disk SQLite**, **164 MiB in-memory SQLite**, and **593 MiB Node with the whole-file NDJSON loader**. Disk SQLite also benefits from OS page cache outside its process RSS. The array updates changed only memory; disk SQLite updates committed to its temporary database. Therefore the sub-millisecond array update timings do not establish persistent write performance.

## 6. Node.js loading the original files

The previous Node loader read the entire NDJSON file into a string before parsing, which temporarily retained substantial input text and line storage. We compared it with a 1 MiB chunked NDJSON parser and direct reads of the original JSON files. Direct files were read synchronously or with up to 32 async requests using the default Node I/O pool or a 16-thread pool. All loaders ran the same structural queries and joins and produced the same answers. Their queries run on **parsed in-memory objects after loading**, not by reopening each file for each view.

| Musenalm entries plus related collections | Whole NDJSON | Chunked NDJSON | Original files sync | Original files async | Original files async16 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Load all four collections and ID maps | 1.151 s | **0.828 s** | 0.884 s | 0.908 s | 0.998 s |
| Reload entries only | 0.911 s | **0.574 s** | 0.646 s | 0.673 s | 0.668 s |
| RSS after all four collections | 593 MiB | **329 MiB** | 341 MiB | 364 MiB | 368 MiB |
| Peak RSS over full run, including reload | 737 MiB | **493 MiB** | 493 MiB | 510 MiB | 518 MiB |

Building the entries NDJSON from original files took another 1.099 s; preparing the three related NDJSON files took 0.165 s. Thus a **first** cache build plus chunked Node load was roughly **2.09 s**, versus **0.88 s** for direct synchronous loading in this warmed-OS-cache experiment. With a cache already available, chunked loading was slightly faster. Direct loading sees current files at every startup without maintaining a separate snapshot.

| 200k original small files | Whole NDJSON | Chunked NDJSON | Original files sync | Original files async | Original files async16 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Load records | **0.201 s** | 0.265 s | 1.436 s | 1.780 s | 1.898 s |
| Reload records | **0.142 s** | 0.210 s | 1.258 s | 1.884 s | 2.152 s |
| RSS after load | 133 MiB | **130 MiB** | 267 MiB | 272 MiB | 273 MiB |

Preparing this NDJSON once took 1.244 s. First preparation plus whole-file Node loading was roughly **1.44 s**, close to the **1.44 s** direct-file load; later starts strongly favor the existing NDJSON cache. On this machine, bounded async per-file reads did not beat synchronous reads, and increasing the I/O pool did not help the 200k case. The earlier 593 MiB Node figure is substantially a consequence of the whole-file input method: direct-file and chunked-cache loading used about 329–341 MiB after all four Musenalm collections. Node still holds those parsed objects in memory during queries.

## 7. Full Musenalm Node.js load and queries

The earlier four-collection result was incomplete for Musenalm. The follow-up [full-data runner](crates/bench/src/bin/musenalm_node_full.mjs) loads all seven directories under `examples/musenalm/data`, including the unregistered `contents`, `items`, and `pages` directories. That is **148,486 JSON files**, with 341.9 MiB of apparent source bytes and about 765.3 MiB of allocated file blocks on this filesystem. The generated compact NDJSON files total 269.2 MiB. The `entries` data already embeds 133,948 content records with `source_id` references, all of which resolve to the standalone content files. Loading both forms keeps both representations in memory.

Three fresh Node processes per loader ran the same queries, with one warm-up and seven timed executions per query. Query results matched exactly by digest. The OS page cache was warm and uncontrolled; startup of the Node process was outside the load timer.

| Load and memory | Original files, sync | Original files, 32 async reads | Existing chunked NDJSON |
| --- | ---: | ---: | ---: |
| Load all seven directories | **2.501 s** | 2.699 s | **1.477 s** |
| Load `contents` alone | 1.480 s | 1.681 s | **0.653 s** |
| Build ID maps | 0.089 s | 0.096 s | 0.086 s |
| RSS after load and explicit garbage collection | 575 MiB | 603 MiB | 493 MiB |
| Peak RSS through queries | 611 MiB | 645 MiB | 538 MiB |

These query times are for **in-memory, hand-written JavaScript after the full load**. They do not reopen files per query:

| Query over full Musenalm data | Direct files | Existing NDJSON |
| --- | ---: | ---: |
| Count contents in 1800–1899 (95,153 matches) | 4.7 ms | 4.9 ms |
| Filter edited contents, sort title/ID, return 50-row page | 180.5 ms | 166.5 ms |
| Facet content types (24 groups) | 18.6 ms | 17.9 ms |
| Search content titles for `almanach` (909 matches) | 27.2 ms | 21.9 ms |
| Search titles plus annotations (1,919 matches) | 47.9 ms | 42.6 ms |
| Resolve 133,948 entry-to-content links with ID map | 29.4 ms | 31.8 ms |
| Top content contributors joined to agents | 35.3 ms | 36.4 ms |

Materializing one filtered title order took about **0.17 s**; paging that saved order then took around **0.005–0.006 ms** in this microbenchmark. That saved order is specific to the filter and must be updated when source records change. It does not provide arbitrary dynamic sorts for free.

Building the full NDJSON cache from original JSON took **3.37 s**. Walking and `stat`ing every source file took **0.596 s** without parsing it. First cache build plus load was therefore about **4.84 s**, compared with **2.50 s** for direct loading. On a later warm start, cached load plus this basic source scan was about **2.07 s**, only about **0.43 s** faster than direct loading. A production freshness check would also compare saved stamps and handle missed changes. No source files were edited by this benchmark.

The correction is important: Musenalm already has nearly 150,000 JSON files, so the earlier statement based on its four registered collections was too narrow. Direct loading still worked and used under 0.6 GiB RSS after loading in this experiment. The cache made repeated loading faster, but its benefit narrowed when source scanning was included. Query latency depends more on the operation and its indexes or saved views than on whether the same objects came from original files or NDJSON.

## 8. Rust versus Node.js on the same Musenalm queries

The Node results above cannot decide the desktop UI language: they compare tailored JavaScript with general-purpose qj and SQL queries. We therefore added direct-file Rust baselines that run the same full-data content views. Each result matched the Node result. The figures below are medians of three fresh processes, with one warm-up and seven timed runs per query. All direct-file loads used a warm, uncontrolled OS page cache.

| Direct-file representation | Load all 148,486 files | RSS after load | Content title page | Content type facets |
| --- | ---: | ---: | ---: | ---: |
| Node.js, all JSON fields retained | 2.501 s | 579 MiB before GC; 575 MiB after | 180.5 ms | 18.6 ms |
| Rust `serde_json::Value`, all fields retained | 2.965 s | 1,520 MiB | 425.9 ms | 73.9 ms |
| Node.js, only view fields retained | 2.524 s | 304 MiB | 128.1 ms | 10.2 ms |
| Rust typed projection, only view fields retained | **1.246 s** | **86 MiB** | **54.3 ms** | **8.6 ms** |

The projected Rust parser deserializes directly into structures containing content year, title, annotation, type, contributor IDs, entry content/item links, and agent names. The projected Node loader first parses each entire JSON object, then keeps the same fields. Both still open and parse **every** source file for this test. They do not represent a lazy startup yet. The generic Rust baseline retains every JSON field as `serde_json::Value`, whose nested object and allocation costs are substantial here; it is not a speed limit for Rust.

| Query on projected fields | Node.js | Rust |
| --- | ---: | ---: |
| Count contents in century | 3.9 ms | **1.4 ms** |
| Build filtered title order once | 143.2 ms | **54.0 ms** |
| Search content titles for `almanach` | **17.3 ms** | 21.1 ms |
| Search titles and annotations | **38.5 ms** | 63.5 ms |
| Resolve all entry-to-content links | **11.5 ms** | 24.4 ms |
| Join top content contributors to agent names | **24.0 ms** | 28.6 ms |

Rust wins several operations; Node wins others. These are implementation measurements, not an inherent ranking of languages: the Rust string search allocates lowercased strings, and the link loops use different object layouts. **Neither projected runner implements the MVP's generic view query model.** They discard fields and implement each query by hand. They are lower-bound examples for known views, not candidates for the library's general query API.

**This does not force Electron.** A Rust desktop UI can call a general Rust query library directly. We should choose its UI toolkit after the library and CLI can execute ad hoc and named views over any configured JSON collection using the MVP's bounded filter and view operations. A view over a few known IDs can read those source files; a new global filter, facet, sort, or aggregation over all contents still needs to scan the collection or use an existing index. The full-load benchmarks show the cost of eager loading, not a mandatory startup sequence.

## Interpretation for the next architecture step

1. **Build the generic direct-file query API next.** Per the revised [MVP](MVP.md), ad hoc and saved views use one small PocketBase-inspired filter syntax plus field selection, sorting, pagination, relation traversal, and simple grouping. Query execution reads the chosen collection's authoritative files. Opening the repository does not load every collection. A global filter or sort can still read every record in the chosen collection when executed.
2. **Keep persistent caching optional.** The full Musenalm NDJSON load was faster than direct full loading, but adding even a basic source scan narrowed the warm-run advantage to about 0.43 s. A first build plus load was slower. Those numbers do not justify making a cache mandatory. Compare direct end-to-end queries through the generic API against an accelerator before adding one to the normal path.
3. **Use the old engine and fixed-query results only as baselines.** Hand-written Node and typed-Rust projections omit fields. qj and jaq were benchmarked on jq expressions over NDJSON; the MVP no longer requires jq as its user-facing language. The UI choice is separate from the query engine choice.
4. **Add indexes only for measured bottlenecks, without restricting query semantics.** SQLite's year, title, and status indexes paid off sharply, but an index for every possible JSON field adds storage and update cost. An unindexed query must still return the same result by scanning source records.
5. **Keep source freshness and correctness separate from query speed.** Direct queries reread source files and see external changes on the next execution. A long-running process with in-memory reuse still needs invalidation; a persistent accelerator needs reconciliation. Neither in-memory arrays nor SQL JSONB replace JSON Schema validation, atomic source writes, conflict checks, or reference integrity.

## Not yet measured

- A collection-neutral direct-file implementation of the MVP's filter and view specification: cold first view, repeated view, complete output serialization, relation traversal, grouping, and edits made externally.
- Optional in-process reuse or persistent accelerators measured against that direct path, including their invalidation and refresh cost. The fixed-field Rust and Node projection benchmarks still opened every source file.
- Cold disk startup with the OS page cache cleared; Node process startup and full UI rendering.
- A full **seven-collection** comparison of qj, jaq, and SQL engines, plus a source-freshness check beyond walking and `stat`ing the files. The shared runner above compared the 133,948-file `contents` collection; the full Node benchmark covered the seven Musenalm `data` directories, not unrelated JSON configuration files.
- Raw ripgrep over source files and cleaned-text search, including normalizer and search-index build costs.
- Complete library writes to authoritative files, atomic writes, conflict detection, validation, and reference checks.
- qj's specialized tape path, an indexed SQLite representation for nested links, and a production DuckDB cache with schema evolution.
- A reusable benchmark runner that accepts declarative workload files and executes the same generic library API on Musenalm, Lenz, and generated repositories rather than hand-written dataset-specific loops.

## Reproducing the main runs

Run from the repository root. The runners use temporary databases and files; the real example repositories are not modified.

```sh
cargo run --release -p objekte-bench -- --repo examples/musenalm --collection entries --threads 4 --work-dir target
cargo run --release -p objekte-bench --features sqlite-bench --bin sqlite-cache-bench -- --repo examples/musenalm --collection entries --work-dir target --cache-mib 256 --batch 100
cargo build --release -p objekte-bench --features sqlite-bench --bin array-compare
target/release/array-compare --repo examples/musenalm --collection entries --runs 7
target/release/array-compare --synthetic 200000 --runs 7
```

The separate SQLite/Turso/Surreal runner requires the optional `engine-compare` feature; its build is much heavier. See [benchmark README](crates/bench/README.md) for those commands and [detailed results](crates/bench/RESULTS.md) for complete timings, versions, and limitations.
