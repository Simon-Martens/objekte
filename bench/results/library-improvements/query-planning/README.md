# Q1 access-path planning timing

Measured code revisions: baseline `a4ae045cba2c86622bec755ec4011229608be499`; Q1 `dfeb112` (`perf(query): estimate access paths without candidate construction`). Each was compiled and run from its own worktree, sequentially, with Rust 1.99.1 release defaults on an Intel Core i9-13900K (x86-64). The standalone harness source is identical in both runs: `Cargo.toml` SHA-256 `eb336229c1430c2823d7153a033480465cbc449b51a6bf8cc19214667f4a0a2a`, `src/main.rs` SHA-256 `ffc2e6efe6789b3fb2032066ea87e32d5be15a53bd435c64e9875b8cb0d3db0e`.

The harness generates one JSONL file of 8,192 records with 16 numeric fields (`f0` through `f15`, each value `slot % 4`). The generated file SHA-256 was `797f387395a5be63a926ba0881741b86718a4a6fe82c654032be74dadbce4ea9` in both worktrees. Both builds use auto inference, snapshot disabled, verified cache disabled, one build worker, one query worker, locator output, count enabled, and a one-row page. Index construction and one warm query per case are outside the timed region. Each sample times 200 searches against one loaded repository; five blocks were run. Search results were asserted to contain 2,048 matches.

| Conjuncts | Baseline median ms/search (min–max) | Q1 median ms/search (min–max) | Delta ms | Delta % |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 0.121560 (0.120686–0.122781) | 0.115416 (0.112183–0.119445) | -0.006144 | -5.1% |
| 4 | 0.191679 (0.190480–0.194805) | 0.139473 (0.136066–0.142328) | -0.052206 | -27.2% |
| 16 | 0.720725 (0.719765–0.723311) | 0.219449 (0.214583–0.222038) | -0.501276 | -69.6% |

The raw five-block samples are in `baseline-a4ae045.csv` and `final-dfeb112.csv`. This is a focused timing observation, not the full historical query workload. It isolates conjunction planning with complete inferred numeric postings; it does not quantify other predicates, mixed collections, or source-reading queries.

Structural tests assert one inferred term estimate and one executed candidate build per term for 1, 4, and 16 conjuncts; a 16-child OR estimates each term once. Estimated dictionary probes stay bounded. A selective stored-order attempt reuses its filter plan in top-k instead of executing the filter again. Temporal period estimates probe at most 33 dictionary keys in the test fixture, while all six comparison operators retain exact results and restricted-set parity.

Baseline commands from `/home/simon/source/objekte-library-planning-baseline-20261107`:

```sh
cargo build --release --manifest-path bench/results/library-improvements/query-planning/harness/Cargo.toml > bench/results/library-improvements/query-planning/baseline-build.stdout.log 2> bench/results/library-improvements/query-planning/baseline-build.stderr.log
mkdir -p bench/results/library-improvements/query-planning/baseline-fixture
bench/results/library-improvements/query-planning/harness/target/release/objekte-q1-timing bench/results/library-improvements/query-planning/baseline-fixture > bench/results/library-improvements/query-planning/baseline.csv 2> bench/results/library-improvements/query-planning/baseline.stderr.log
```

Q1 commands from `/home/simon/source/objekte-library-query-planning-20261107`:

```sh
CARGO_TARGET_DIR=target-q1-harness cargo build --release --manifest-path bench/results/library-improvements/query-planning/harness/Cargo.toml --quiet
mv target-q1-harness target/q1-harness
target/q1-harness/release/objekte-q1-timing bench/results/library-improvements/query-planning/fixture > bench/results/library-improvements/query-planning/final-dfeb112.csv
```
