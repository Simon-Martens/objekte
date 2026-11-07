# Inferred posting sort: Musenalm check

Measured code: `9a8ac785bd72f8a3471051e9d338fb80921b4431` on `development/local-file-search-core`. The before sample was taken immediately before that sort change, after exact inferred scalar predicate proofs at `ccf6d60761`. Both used `objekte-cli` release builds, `rustc 1.99.1`, Linux x86_64, the local `examples/musenalm` corpus (148,488 files and records), default ASCII alphabet, automatic indexing, no schema, and an in-memory fresh build. Each row is one invocation; no variability claim is possible.

Command (without `--count` for before and after):

```sh
./target/release/obj --path examples/musenalm --auto-index --limit 10 'year >= 1800 SORT BY year DESC'
```

The third run adds `--count`. Raw NDJSON and stderr are saved beside this note.

| Run | Search ms | Difference from before | Candidates | Source opens | Record parses | Sort key evaluations | Returned | Exact total |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Before | 1,395.31 | — | 98,974 | 98,974 | 98,974 | 98,974 | 10 | not requested |
| After | 9.78 | -1,385.53 ms (-99.30%) | 98,974 | 10 | 0 | 0 | 10 | not requested |
| After, `--count` | 19.50 | -1,375.81 ms (-98.60%) | 98,974 | 10 | 0 | 0 | 10 | 98,974 |

The ten returned record IDs and their order match exactly across all three runs. The ten source opens in the new runs load the requested full payloads; locator pages use no source reads in focused tests. The old run sorted by loading every matching record. The new run traverses the complete inferred numeric posting order and uses exact posting membership. The measured `build_ns` values were 2,038.23 ms before and 2,099.35 ms after; these single fresh-build samples do not establish a build-time change.

The optimized path requires one selected collection and a complete scalar inferred posting for a simple sort field. Partial short-string coverage, arrays, transformations and source-dependent predicates keep the existing computed sort. Distinct string keys require ICU collation, so their query-local order is derived from the posting dictionary rather than assuming UTF-8 key order equals locale order.

Checks at the measured code revision: `cargo test --workspace` passed, including independent reference parity for numeric, Boolean and short-string sorts, null/missing/fallback handling, offset/count, stream and cache-enabled routes. `cargo clippy --workspace --all-targets` passed with existing warnings. Edited files passed `rustfmt --check` and `git diff --check`; repository-wide `cargo fmt --check` still reports formatting differences in untouched files.
