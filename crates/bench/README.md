# Objekte cache and embedded query benchmark

These are historical experiments. The former core NDJSON cache is preserved here
as `src/legacy_cache.rs` so its measurements remain reproducible. The new library
and CLI are verified separately through the [library acceptance suite](../core/tests/README.md).

The [28 October 2026 rerun](RERUN-2026-10-28.md) compares direct Rust, Node full objects, Rust SQLite JSONB on disk, and embedded qj across several collections and Musenalm nested workloads.

The current library has separate reports for [Sonic parsing](SONIC-RS-2026-10-28.md)
and [first-cache ingestion, phase timings, and Turso versions](INGESTION-2026-10-28.md).

## Shared scalar workload across current options

`shared-query-compare` accepts field names at runtime and checks four matching operations across embedded qj, embedded jaq, Rust full JSON, Rust field projection, SQLite JSONB in memory or on disk, and Node. `shared_query_compare_jq.py` runs the same queries through a fresh jq subprocess. Results and limitations are at the top of [Performance.md](../../Performance.md). The source files are only read. The NDJSON output and SQLite database are disposable generated artifacts.

```sh
cargo build --release -p objekte-bench --features sqlite-bench --bin shared-query-compare
mkdir -p target/shared-bench
target/release/shared-query-compare --engine prepare --source-dir examples/musenalm/data/contents --output target/shared-bench/contents.ndjson
target/release/shared-query-compare --engine scan --source-dir examples/musenalm/data/contents
target/release/shared-query-compare --engine rust-source --source-dir examples/musenalm/data/contents --number-field year --text-field preferred_title --lower 1800 --upper 1899 --needle almanach --runs 3
target/release/shared-query-compare --engine rust-projected-source --source-dir examples/musenalm/data/contents --number-field year --text-field preferred_title --lower 1800 --upper 1899 --needle almanach --runs 3
target/release/shared-query-compare --engine qj --input target/shared-bench/contents.ndjson --number-field year --text-field preferred_title --lower 1800 --upper 1899 --needle almanach --runs 3
target/release/shared-query-compare --engine jaq --input target/shared-bench/contents.ndjson --number-field year --text-field preferred_title --lower 1800 --upper 1899 --needle almanach --runs 3
target/release/shared-query-compare --engine sqlite-memory-source --source-dir examples/musenalm/data/contents --number-field year --text-field preferred_title --lower 1800 --upper 1899 --needle almanach --runs 3
node crates/bench/src/bin/shared_query_compare.mjs --source-dir examples/musenalm/data/contents --number-field year --text-field preferred_title --lower 1800 --upper 1899 --needle almanach --runs 3
python3 crates/bench/src/bin/shared_query_compare_jq.py --input target/shared-bench/contents.ndjson --number-field year --text-field preferred_title --lower 1800 --upper 1899 --needle almanach --runs 3
```

Other Rust modes are `rust`, `rust-source`, `rust-projected`, `sqlite-memory`, `sqlite-disk`, `sqlite-disk-source`, `sqlite-open`, and `sqlite-indexed`. NDJSON modes take `--input`; source modes take `--source-dir`; disk modes take `--output` with a database path. `sqlite-indexed` opens an existing database and builds a numeric-field expression index if missing; that mode currently accepts only simple ASCII alphanumeric or underscore field names. Use a **new** `--output` path for each disk-build run. The measured operations assume an integer numeric field and a text field. The benchmark is collection-neutral in field names but still hardcodes four query shapes; it does not replace the MVP's future declarative-workload benchmark of the general library API.
Pass `--projected true` to the Node runner to retain only the requested numeric and text fields, matching the Rust projected runner's retained fields.

These runners record exploratory engine and Musenalm-specific workloads. The revised [MVP](../../MVP.md) requires a separate **collection-neutral** benchmark runner that takes a repository and declarative view workload, exercises the same generic library query API as the CLI, and runs across Musenalm, Lenz, and generated repositories. The existing hand-written query loops do not satisfy that decision gate.

Run against a repository collection:

```sh
cargo run --release -p objekte-bench -- --repo examples/musenalm --collection entries --threads 4
```

Run with 200,000 generated JSON source files:

```sh
cargo run --release -p objekte-bench -- --synthetic 200000 --threads 4 --work-dir target
```

Pass `--filter 'jq expression'` one or more times to compare independent-record filters. Pass `--global 'jq expression'` for sorting, grouping, or other expressions that consume the whole collection as one array. Without custom expressions, the runner uses `.id`, a guarded year filter, a global sort, a global group, and a filtered global sort. `--no-global` skips the collection-wide queries.

The runner creates its cache and synthetic source files in a temporary directory and removes them afterward. By default this is the system temporary directory, which may be RAM-backed. Use `--work-dir PATH` to choose a filesystem for the cache and generated sources; the example uses the project's `target` directory on disk. It reports source generation separately from the cache build. It measures cold build, an unchanged full rebuild, source-inventory scans, a one-record refresh that rewrites one of 64 NDJSON shards, opening the published generation without an index, and loading the full ID index. For synthetic input it modifies one source file before the refresh. The real repository is never edited.

The first measured results are in [RESULTS.md](RESULTS.md).

Both query engines are embedded. Independent-record filters use each engine's public input reader and parser, one compiled filter per worker, the same shard files and thread count, and count outputs. Global queries read all records into each engine's own array value, then run one jq expression. The runner reports input slurping, query execution, and their sum separately. Global queries use one thread and retain the collection in memory; `--threads` applies only to independent-record filters. Global result values are serialized and compared for equality. Independent-record filters compare output counts only. qj's specialized tape execution path is not measured, so these timings compare the ordinary embedded APIs rather than every optimization either engine can offer.

## SQLite JSONB cache lifecycle

The optional Rust benchmark builds a separate SQLite database directly from JSON source files, without an NDJSON staging cache:

```sh
cargo run --release -p objekte-bench --features sqlite-bench --bin sqlite-cache-bench -- --synthetic 200000 --work-dir target --cache-mib 256 --batch 1000
cargo run --release -p objekte-bench --features sqlite-bench --bin sqlite-cache-bench -- --repo examples/musenalm --collection entries --work-dir target --cache-mib 256 --batch 100
```

It supports JSON collections with filename IDs. Each temporary database has `records(id, data)` with SQLite JSONB and a separate `sources` table for path, size, and modification time. It uses WAL, `synchronous=NORMAL`, and the selected SQLite page-cache limit. It times a fresh build, a full rebuild into a new database file, source reconciliation scans, a one-record refresh, an expression index for `$.year`, a batched refresh, and WAL checkpoints. Synthetic input is edited between scans. For real repositories, the benchmark creates modified copies in its temporary working directory, so source repositories are not changed. Temporary databases and generated files are removed when the run ends.

The SQLite crate is bundled only with the `sqlite-bench` feature; the ordinary qj/jaq benchmark does not compile it. Results are in [RESULTS.md](RESULTS.md).

## Embedded SQLite, Turso, and SurrealDB comparison

Build the optional three-engine runner, then run each engine separately on the same collection or synthetic input:

```sh
cargo build --release -p objekte-bench --features engine-compare --bin engine-compare
target/release/engine-compare --engine sqlite --repo examples/musenalm --collection entries
target/release/engine-compare --engine turso --repo examples/musenalm --collection entries
target/release/engine-compare --engine surreal --repo examples/musenalm --collection entries
target/release/engine-compare --engine sqlite --synthetic 200000
target/release/engine-compare --engine turso --synthetic 200000
target/release/engine-compare --engine surreal --synthetic 200000
```

`--work-dir PATH` places all generated source files and temporary databases on that filesystem; the default is `target`. The runner leaves real repository files untouched. It reads source JSON before starting each engine's ingestion timer, prints query-result digests for comparison, and reports filter, sort, group, nested-agent lookup, year-index creation, and cached-record updates. On a collection with nested `agents`, it also builds a SurrealDB index on `data.agents.*.agent`, checks the query plan, and verifies that the index reflects a subsequent record update. The three engine dependencies are optional and do not compile in the default benchmark. See [RESULTS.md](RESULTS.md) for measurements and limitations.

## In-process arrays versus SQLite JSONB

`array-compare` measures qj, SQLite JSONB on disk and in `:memory:`, and five Node.js loading methods. Node can parse the whole NDJSON file, parse NDJSON in 1 MiB chunks, or discover and read the authoritative JSON files directly. Direct-file reads are tested synchronously and with 32 bounded async requests using Node's default I/O thread pool or a 16-thread pool. Node.js must be on `PATH`. The runner benchmarks initial load, rebuild, seven warm executions of each query, a year-sorted materialized view or SQLite expression index, point and batch updates, and process memory. It checks full query results for equality across engines. Run it on the large nested collection and a high-record-count synthetic collection:

```sh
cargo build --release -p objekte-bench --features sqlite-bench --bin array-compare
target/release/array-compare --repo examples/musenalm --collection entries --runs 7
target/release/array-compare --synthetic 200000 --runs 7
```

The runner reads source JSON and creates one compact NDJSON file first; that preparation time is reported separately. Direct-file Node reads do not use that cache, but source preparation before their turn can warm the OS file cache. On Musenalm entries, the runner also loads the registered agents, series, and places collections and runs title paging, review-status facets, combined nested filters, full title-search counts and pages, content-type and role aggregates, and cross-collection joins. It measures SQLite's title and review-status expression indexes separately and verifies that SQLite uses them. Synthetic source-file creation is separate. Each engine runs in its own process so RSS figures are meaningful. A qj or Node.js “year view” is a saved sorted result for one filter; SQLite's expression index is reusable for matching queries. The array update timings do not include persistence, while disk SQLite commits to its temporary database. SQLite's cached reopen timing assumes an existing database and a warm OS file cache. See [RESULTS.md](RESULTS.md) for the measured numbers and comparison details.

## Node.js across the full Musenalm data tree

The separate runner below includes the 133,948 standalone `contents` files and all six other `data` directories, including those unregistered in `.editor/repository.json`. It compares direct synchronous reads, 32 bounded async reads, and an existing compact NDJSON cache. It also times building the cache and walking/`stat`ing every source file. Queries cover content filtering and sorting, facets, title and annotation substring search, entry-to-content and entry-to-item links, and content contributors joined to agents. The runner prints JSON lines with timings, memory, result counts, and digests. Source files are read-only; `build-cache` writes only to the selected cache directory.

```sh
node --expose-gc --max-old-space-size=8192 crates/bench/src/bin/musenalm_node_full.mjs --repo examples/musenalm --mode build-cache --cache target/musenalm-node-cache
node --expose-gc --max-old-space-size=8192 crates/bench/src/bin/musenalm_node_full.mjs --repo examples/musenalm --mode scan --cache target/musenalm-node-cache
node --expose-gc --max-old-space-size=8192 crates/bench/src/bin/musenalm_node_full.mjs --repo examples/musenalm --mode original --cache target/musenalm-node-cache
node --expose-gc --max-old-space-size=8192 crates/bench/src/bin/musenalm_node_full.mjs --repo examples/musenalm --mode original-projected
node --expose-gc --max-old-space-size=8192 crates/bench/src/bin/musenalm_node_full.mjs --repo examples/musenalm --mode original-async --cache target/musenalm-node-cache
node --expose-gc --max-old-space-size=8192 crates/bench/src/bin/musenalm_node_full.mjs --repo examples/musenalm --mode ndjson --cache target/musenalm-node-cache
```

`scan` measures source enumeration and stat calls only. It is not a complete freshness check. The loader holds all parsed records in memory before queries, so the query numbers do not include disk reads. These Node operations are hand-written for the measured views; they are not equivalent to arbitrary user-defined queries in qj or SQL. Detailed results are in [RESULTS.md](RESULTS.md#full-musenalm-nodejs-data-load-including-standalone-contents).

The projected Node mode keeps only the fields used by these views. Two Rust runners read the same original files and run matching queries: one retains generic `serde_json::Value` trees, and the other deserializes only view fields into Rust structs. This tests whether the fast Node queries imply an Electron UI. All four modes still read every source file; none is a truly lazy loader.

```sh
cargo build --release -p objekte-bench --bin musenalm_rust_full --bin musenalm_rust_projected
target/release/musenalm_rust_full --repo examples/musenalm --runs 7
target/release/musenalm_rust_projected --repo examples/musenalm --runs 7
```

The [full-data Rust/Node comparison](RESULTS.md#full-musenalm-direct-file-rust-and-nodejs-comparison) reports the measurements and the representation differences.
