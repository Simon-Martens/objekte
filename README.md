# Objekte

Objekte is being rebuilt as a Rust library with a command line application. The MVP queries authoritative repository files directly and supports configurable views over any configured JSON collection, using a small PocketBase-inspired filter language. The CLI exercises library functions before a new UI is built.

The `examples/` directory contains sample file repositories with JSON Schema and UI configuration. The previous Rust backend, its web assets, and their documentation are preserved in [`deprecated/`](deprecated/README.md) as design and behavior references. The former `frontend/` directory contains generated artifacts only; its source was already absent from the repository.

The [new MVP](MVP.md) defines the library-first rebuild and its three access modes. [Performance.md](Performance.md) summarizes the cache, database, qj/jaq, Node.js, and Rust experiments. The first implemented library feature is an indexed, sharded collection cache experiment; it is **not required by the MVP**. [`crates/bench`](crates/bench/README.md) contains the runnable benchmarks and detailed results.

## Current workspace

| Path | Purpose |
| --- | --- |
| `crates/core` | Rust library, currently with experimental collection cache construction and indexed refresh |
| `crates/cli` | `objekte` command line entry point |
| `crates/bench` | Cache and embedded query benchmark runner |
| `examples` | Sample repositories for future CLI commands |
| `deprecated` | Archived backend, web interface, and documentation |

The current cache experiment can be run with `cargo run -p objekte-cli -- cache build examples/project people`; `cache changes` scans for external source changes. See [`crates/bench/README.md`](crates/bench/README.md) for reproducible cache and embedded query timings. Check the workspace with `cargo check --workspace`.

The next product work is a collection-neutral direct-file query API and the two ripgrep-based search modes. Existing Musenalm benchmarks are workload examples; the MVP requires a reusable benchmark runner using declarative queries across multiple repositories. The cache experiment's targeted refresh handles changed or deleted records with known IDs; new records and ID changes still require a full rebuild.
