# Full-folder Musenalm AutoIndex acceptance

Measured code: `4319910fba83dbab63922717afa159a1ec4d6860` on
`development/local-file-search-core`. The earlier failure was observed at
`b7907d8cf3bd1000d8ae480cdf05f212d7afa1b6` (`before.ndjson`). These are
single diagnostic runs, not a repeatable speed distribution.

Environment: Linux `7.2.13-200.fc44.x86_64`, 32 available CPUs,
`rustc 1.99.1`, release profile, default ASCII alphabet, no snapshot, one
`files` collection rooted at `examples/musenalm`. The collection contains
148,488 JSON files, including two JSON files outside the seven data folders.
`queries.txt` contains the two query expressions. The executable was built
before timing with `cargo build --release -p objekte-cli --bin obj`.

The measured invocations used the same `--count --limit 1 --query-file`
options, with `--auto-index` added only for the second run. `/usr/bin/time`
captured peak process RSS in the `.time` files. `build_ns` and `search_ns` below
come from the command's NDJSON events, not external process duration.

| Mode | Build ms | Peak RSS MiB | Inference | Admitted paths |
| --- | ---: | ---: | --- | ---: |
| Disabled | 773.34 | 390.90 | disabled | 0 |
| AutoIndex | 2,073.24 | 591.34 | complete | 308 |

| Query | Exact matches, both modes | Disabled candidates / parses / search ms | AutoIndex candidates / parses / search ms |
| --- | ---: | ---: | ---: |
| `year = 1800` | 2,989 | 148,488 / 148,488 / 516.46 | 2,989 / 2,989 / 18.42 |
| `year >= 1800 && year < 1810` | 24,756 | 148,488 / 148,488 / 516.76 | 64,658 / 64,658 / 242.71 |

AutoIndex admitted 275 paths with short-string postings, of which 275 have
residual coverage, plus other compatible paths including numeric `year`.
Three paths exceeded a per-path budget and 74 had incompatible observed shapes
or kinds. The complete observation report does not mean every path was indexed.
The new defaults are bounded estimates: 16 MiB per batch, 512 MiB coordinator
retention and 768 MiB repository work allowance. Policy version 4 invalidates
snapshots made under the earlier defaults.

The original whole-folder AutoIndex run had `complete=false`, zero observed
paths and zero admitted indexes. Several 512-file agent batches exceeded the
old 4 MiB batch estimate. With that limit raised alone, the coordinator then
crossed the old 128 MiB test limit near record 40,448. The previous normal
default was 64 MiB; both stages needed sufficient allowance for this fixture.

The earlier *entries-only* benchmark is a different comparison: its six-query
batch took 112.76 ms with manual schema indexes and 137.07 ms with AutoIndex.
The 137.07 ms AutoIndex result was faster than the 194.80 ms disabled result,
not faster than manual indexing. Manual `year` postings can prove exact
membership without source parses; inferred numeric postings still retain
residual verification.

Verification: `cargo test --workspace` passed. The full-folder CLI acceptance
test was run explicitly and passed; it is ignored in routine test runs because
the debug build takes about two minutes on this fixture. The smaller dense
numeric batch regression runs normally. `cargo clippy --workspace --all-targets`
completed with pre-existing warnings, and the changed Rust files passed
`rustfmt --check`.
