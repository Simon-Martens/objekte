# Construction scaling probes

These are single release-build samples from 2026-11-07. The fixture source is
[`bench_construction_scaling.rs`](bench_construction_scaling.rs). The baseline
JSON files (`schema-*x1.json`, `sparse-*.json`, and `shared-*.json`) were
measured from `a4ae045cba2c86622bec755ec4011229608be499`. The
`schema-final-*.json` files were measured from the source later committed as
`b8b2d3f93d4f9005a468162f3b428994425ed751`. An intermediate after-state
was excluded because the code changed again before the final measurement.

## Setup and commands

- Rust `1.99.1` (`58cf8f9c3`, 2026-10-13), Cargo release profile with no
  recorded `RUSTFLAGS`, target override, or release profile override.
- Fedora Linux 44, kernel `7.2.13-200.fc44.x86_64`, x86-64,
  Intel Core i9-13900K (32 logical CPUs). Measurements were run sequentially
  in an uncontended window; CPU affinity and filesystem cache were not fixed.
- The harness uses one explicit build worker for repository fixtures,
  `SnapshotMode::Disabled`, and otherwise default `RepositoryOptions`.
  The schema fixture only registers in-memory fields, so it has no worker or
  snapshot setting. Compilation is outside every reported interval.

From the repository root at the baseline revision, place the fixture source at
`crates/core/examples/bench_construction_scaling.rs`, change to this result
directory, then run (the relative binary path starts at the result directory):

```sh
# Compile from the repository root before changing directory:
# cargo build --release -p objekte-search-core --example bench_construction_scaling
for n in 250 500 1000 2000 4000; do
  ../../../../target/release/examples/bench_construction_scaling schema "$n" > "schema-${n}x1.json"
done
for n in 32 64 128; do
  ../../../../target/release/examples/bench_construction_scaling sparse "$n" 64 > "sparse-${n}x64.json"
done
for n in 1 4; do
  ../../../../target/release/examples/bench_construction_scaling shared "$n" 512 > "shared-${n}x512.json"
done
```

At the final revision, compile the same example again and run only the schema
loop, writing `schema-final-${n}.json` files. The original invocations were
made from the repository root with `./target/release/examples/...` and
redirection into `target/construction-results/`; the paths above produce the
versioned copies directly. No test invocation is timed by this harness.

The sparse and shared probes ran only at the baseline revision. The fixture
creates its temporary JSON files under `target/` and removes them after each
repository build. `schema` measures `Schema::register_field` for distinct Date
fields. `sparse` creates separate collections of 64 JSON files each. `shared`
registers the same 512 physical JSON files in one or four collections.

## Observations

| Date fields | Baseline registration ms | Final registration ms |
| ---: | ---: | ---: |
| 250 | 7.485 | 0.169 |
| 500 | 20.939 | 0.290 |
| 1,000 | 65.031 | 0.557 |
| 2,000 | 260.441 | 1.305 |
| 4,000 | 1,105.366 | 4.333 |

At 4,000 fields, the recorded process peak RSS was 10,972 KiB before and
7,828 KiB after. These probes isolate registration; they do not measure
Musenalm or overall repository construction. Individual small differences are
not statistical claims.

The 128-collection sparse probe had 8,192 physical files and records, 97,024
input bytes, 8,192 parses, 48.398 ms repository build time, 4,227,584 bytes
of flat file-record mapping capacity, 2,228,337 estimated file-inventory heap
bytes, and 16,248 KiB peak RSS. The mapping estimate sums `Vec` capacities,
not allocator overhead. Sparse mapping representation changes were deferred
because they touch snapshot and validation contracts.

The shared-source probes had 512 physical files and 6,546 input bytes. One
collection yielded 512 parses and 4.878 ms repository build time; four
collections yielded 2,048 parses and 9.211 ms. Shared parse reuse was deferred
because it requires broader reader, schema, and collection semantics work.
All detailed phase timings, counts, and size estimates are in the raw JSON.
