# Direct snapshot access: pre-change baseline

The task branch `integration/direct-snapshot-access` starts at `dd3f8feb19df68dd5a5a3ada0d8c99ca91336524` on `integration/hybrid-index-storage`. The original checkout remains on master. The authoritative task document was untracked in that checkout and is preserved there until completion.

Environment: Linux 7.2.15-200.fc44.x86_64, Intel Core i9-13900K, Rust 1.99.1. Release examples were compiled with `--locked`. The common fixture is `/home/simon/source/objekte-integration-hybrid-index-storage/examples/musenalm` (148,486 source reads during priming). These are warm, source-free managed opens, not cold-cache measurements. The runner verifies zero source reads and zero build parses for every open and compares result digests across trials. Each median below uses seven runs per mode and alphabet.

| Mode/profile | Open ms | First Q167 ms | Resident MiB | Managed MiB |
| --- | ---: | ---: | ---: | ---: |
| On-demand ASCII | 386.805 | 8.954 | 271.9 | 125.0 |
| On-demand Unicode | 404.822 | 10.247 | 543.9 | 127.7 |
| Resident ASCII | 451.814 | 8.082 | 323.6 | 125.0 |
| Resident Unicode | 503.774 | 9.228 | 544.5 | 127.7 |

Fresh incremental ASCII build median: 1,166.698 ms; first batch median: 353.099 ms; after-build RSS median: 574.9 MiB (seven runs). The build runner uses 32 workers. The exact per-run commands and raw samples are in `baseline-dd3f8f/warm-opens.json` and `baseline-dd3f8f/builds.json`. The runners are under `scripts/` and were invoked as follows after compiling and copying the base binaries into ignored `target/direct-snapshot-access/baseline/`:

```sh
cargo build --release --locked -p objekte-search-core --example bench_musenalm
cargo build --release --locked -p objekte-search-core --example bench_index_construction
python3 bench/results/direct-snapshot-access/scripts/run_warm_opens.py --program target/direct-snapshot-access/baseline/bench_musenalm --root /home/simon/source/objekte-integration-hybrid-index-storage/examples/musenalm --store target/direct-snapshot-access/baseline/store --output target/direct-snapshot-access/baseline/warm-opens --label dd3f8feb19
python3 bench/results/direct-snapshot-access/scripts/run_build_trials.py --program target/direct-snapshot-access/baseline/bench_index_construction --root /home/simon/source/objekte-integration-hybrid-index-storage/examples/musenalm --output target/direct-snapshot-access/baseline/builds --label dd3f8feb19
```

The copied `bench_musenalm` binary hashes to `18ade062d7f819f553179e5a721ecffda1c4242d61c586fc00da5547cdb89228` and `bench_index_construction` to `b71dc082eeafc0b9cfc4e41ac94b1436c365a7a02fccdfb8f61e103246e44ba1` (SHA-256). The store was primed once with the same benchmark command using the source fixture and a fresh result directory.

## Starting traversal and lookup inventory

- Dictionary opening verifies entries and currently constructs a gram filter while visiting them. Stamped opening still reconstructs a filter from encoded keys. Sealing an already sealed dictionary can rescan it for a filter.
- Segment record-slot decoding allocates `SegmentFile` and `RecordEntry` arrays, a file ID map, and per-file slot lists. `validate_records` then revisits the records and derives an ID map and logical/fallback domains.
- Catalog decode allocates all current and historical entries and sorts paths for duplicate detection. Restoration builds changed-file and changed-path maps and an ordered path tree from that decoded catalog.
- Generation restoration scans active records and builds a cloned-ID set for cross-segment uniqueness. This is a correctness check distinct from lookup reconstruction.
- `SegmentReader.section()` verifies an entire section checksum before exposing bytes; a borrowed view alone does not mean opening avoids touching the section.

The byte-level format contract and validation inventory will be documented before decoder changes. This baseline is a measurement checkpoint, not a claim that the task's structural or performance acceptance is met.
