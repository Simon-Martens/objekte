# Musenalm benchmark: merged master

- Source revision: `d35ddde56e17114da35d1a76e776615262897409`
- Corpus: `/home/simon/source/objekte/examples/musenalm`
- Toolchain: Rust 1.99.1, Cargo 1.99.1
- Host: 13th Gen Intel Core i9-13900K, 32 logical CPUs

## Query workload

```sh
OBJEKTE_BUILD_REVISION=d35ddde56e17114da35d1a76e776615262897409 \
  cargo build --locked --release -p objekte-search-core --example bench_musenalm

target/release/examples/bench_musenalm \
  --root /home/simon/source/objekte/examples/musenalm \
  --profiles ascii-off --repetitions 3 --query-workers 4 \
  --verified-cache-bytes 0 \
  --result-dir bench/results/musenalm/master-d35ddde5-20261119/queries
```

All 150 cases completed successfully across 450 samples. The median of the
150 per-case medians was 416.763 µs. The slowest cases were Q007 at
339.328 ms and Q006 at 328.876 ms.

Raw measurements, plans, resolved queries, ordered-result digests and the run
manifest are under `queries/`.

## Construction

```sh
target/release/examples/bench_build_musenalm \
  --root /home/simon/source/objekte/examples/musenalm \
  --alphabet <ascii|unicode> --build-workers auto \
  --schema-manifest-path bench/results/musenalm/master-d35ddde5-20261119/build/schema-manifest.json \
  >> bench/results/musenalm/master-d35ddde5-20261119/build/measurements.jsonl
```

Three samples were run for each alphabet with 32 resolved build workers:

| Alphabet | Median | Range | Peak RSS median |
|---|---:|---:|---:|
| ASCII | 734.620 ms | 704.630–758.874 ms | 623.8 MiB |
| Unicode | 769.554 ms | 750.196–780.187 ms | 633.0 MiB |

Each sample built 148,486 records with 148,486 parses and zero failed parses.
The ASCII index signature is
`4a38808629a1544a354d715e42c85ac426b1cfe93376e862253b3d5cc03f349f`;
the Unicode signature is
`5c235a33b6cb1a18c1a1f4d2b1d60fa1eae912ce2b4ff868857df95028d83868`.

`summary.json` contains the exact samples, medians, ranges and slowest-query
list.
