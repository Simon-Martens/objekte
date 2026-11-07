# Q5 exact membership, focused release comparison

The pre-change code is `f14fb9b8e211337a8d77bbc055d251322ce85713`.
The changed code is `05daf7935ed9d2b1ccfe1466d9c520a35f191b40`.
Both used the same public-API harness: SHA-256 `a4f006adad8332504da0b724fb6775ef231bb80ff1401955e2a34e50623a77ab` for `harness/Cargo.toml` and `6f48754b64bfb2231ddee5946652cebd01512f45715a7b0248c23aa53ad14427` for `harness/src/main.rs`.
The generated `harness/Cargo.lock` was byte-identical in both worktrees (SHA-256 `3f9ec94b368e9798c45645d57c6a3ad7b541155edc6d7c0298dedbd8da8896a7`).

The harness writes one deterministic JSONL file with 12,288 records (454,656 bytes, SHA-256 `9592fc2f4cc8fb02b6cb51f5bae5b1dbd767851d603f922b548054af5d4b1a5d`). The `kind` field has an exact posting index. The `body` field is a valid computed sort key. Snapshot mode and verified-result caching are disabled; query and build workers are both one. The repository is built outside each timed interval. Each process opens five repository instances and times one request of each kind per instance.

| Request | Before median ms | After median ms | Difference ms | Difference % | Before range ms | After range ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Exact count | 0.299 | 0.104 | -0.195 | -65.3% | 0.048 | 0.041 |
| Exact count with `SORT BY body` | 8.507 | 0.087 | -8.421 | -99.0% | 0.148 | 0.046 |
| Exact bitmap selection | 0.421 | 0.086 | -0.335 | -79.7% | 0.050 | 0.042 |

Every request returned 12,288 matches. The sorted count previously opened one source file, parsed 12,288 records and evaluated 12,288 sort keys in each block; afterward those counters are zero. Unsorted count opened no source files in either revision. Selection has no public work counters, so only its cardinality and elapsed time are recorded. The raw five-block samples are `baseline.csv` and `final-05daf793.csv`.

Commands, run sequentially in each worktree, with that worktree as the current directory:

```sh
cargo build --release --manifest-path bench/results/library-improvements/exact-membership/harness/Cargo.toml
mkdir -p bench/results/library-improvements/exact-membership/fixture
bench/results/library-improvements/exact-membership/harness/target/release/objekte-q5-timing bench/results/library-improvements/exact-membership/fixture
sha256sum bench/results/library-improvements/exact-membership/fixture/data/records.jsonl
```

Build output was redirected to the corresponding `*-build.*.log` files; harness output was redirected to the CSV and stderr files. The baseline used a detached worktree at the exact pre-change commit, and the final used the task branch at the exact changed commit. They ran one after the other without overlapping builds or measurements. Compiler: `rustc 1.99.1` release profile, x86-64 Linux `7.2.13-200.fc44.x86_64`, 32 visible CPUs. No CPU pinning or OS cache dropping was used. These are five focused observations on one synthetic fixture, not a full workload speed claim. The large sorted-count difference is explained by the removed source reads, parses and sort-key evaluations; the smaller count and selection differences are consistent with less per-record collection work, without an allocation profile to prove that attribution.
