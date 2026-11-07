# Library improvement execution record

## Starting state and baseline

The integration branch starts from `81788174898c27b2c2c367319bad20ecee450562` on `master` (7 November 2026). The original checkout remains on `master`; its untracked implementation plan is not part of the baseline. Toolchain: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `cargo 1.99.1`. Machine: 32 available CPUs, Linux `performance` governor. All commands below ran in `/home/simon/source/objekte-library-integration-20261107` with a separate Cargo target directory and repository fixture in that worktree.

Baseline validation: `cargo test --workspace` passed; `cargo clippy --workspace --all-targets` exited successfully with pre-existing warnings. Complete logs are in [the baseline evidence](../../bench/results/musenalm/library-improvement-baseline-8178817489/).

The release examples were compiled before timing:

```sh
cargo build --release -p objekte-search-core --example bench_build_musenalm --example bench_musenalm
./target/release/examples/bench_build_musenalm --root examples/musenalm --alphabet ascii --snapshot-mode disabled --build-workers auto --schema-manifest-path .task-results/baseline/build-schema.json
./target/release/examples/bench_musenalm --root examples/musenalm --extended --cases Q035,Q045,Q084,Q127,Q189,Q190,Q194 --profiles ascii-off --repetitions 6 --result-dir .task-results/baseline/queries
```

The raw result files were moved into the linked evidence directory after measurement. Their manifests retain the exact invocation. The fresh build used 32 resolved workers, parsed 148,486 records with zero parse failures, and had a 1,233.968 ms repository-opening wall interval: 249.414 ms discovery, 717.936 ms extraction/indexing, and 254.137 ms finalization. Peak RSS was 490.215 MiB. Index signature: `a6149d2d78f5f4c14970dae09ebd146e206a59c4a717d224afb84aa42fd2cc5a`.

The query checkpoint used the extended schema and fixed ASCII alphabet, no verified-result cache, zero source/decoded cache, 32 query workers, and six repetitions. All seven cases passed. Median times and actual matches were:

| Case | Query category | Median ms | Matches | Source parses |
| --- | --- | ---: | ---: | ---: |
| Q035 | numeric range | 0.037 | 124 | 0 |
| Q045 | numeric range | 0.176 | 1,107 | 0 |
| Q084 | array equality | 0.209 | 4,922 | 0 |
| Q127 | stored composite sort | 0.061 | 203 | 0 |
| Q189 | text filter with inline sort | 6.326 | 7 | 203 |
| Q190 | numeric range with computed sort | 114.186 | 3,821 | 3,821 |
| Q194 | text filter with composite sort | 0.361 | 7 | 203 |

These are first measurements, not speedup claims. Query construction in the harness also saved and restored a snapshot outside the primary timed query samples; its separate output appears in the raw report. Future comparisons must match corpus, schema, profile, workers, and cache conditions. The chosen cases provide a compact checkpoint; they do not establish the requested 1/4/16-term planning scaling, which needs a focused harness or fixture after Q1 implementation.

## Integrated work

| Finding | Task head | Integration merge | Combined validation |
| --- | --- | --- | --- |
| C1 prepared-cache accounting | `3811445dba` | `7c64ddd22d` | Five focused cache tests and workspace tests passed. |
| C2 JSON parser acceptance | `a9309e6c54` | `e15918792b` | Two focused reader differential tests and workspace tests passed. |
| C4 distinct-byte inference summary | `2c7da67595` | `aee8ef0bcc` | Six focused allocation/boundary tests and workspace tests passed. |
| C5 public numeric invariants | `376bf2bcca` | `bc36dfaf08` | Three focused numeric/admission tests and workspace tests passed. |
| C3 composite file-status order | `43cb4e2f12` | `d674a64dd9` | Shared-file sort, reference and restore test plus workspace tests passed. |
| B3 malformed-file coordinates | `449ac02a6d` | `3646f3e84f` | Linear scanned-byte and custom-reader coordinate tests plus workspace tests passed. |
| B2 relation validation | `6ef87fa10e` | `cef862d15d` | Relation corruption tests, independent oracle and workspace tests passed. |
| C6 path/binding contract | `46ea35d80c` | `a4ae045cba` | Quoted-colon schema/FILTER/FTS/sort/restore test, workspace tests and Clippy passed. |
| B4 schema conversion validation | `b8b2d3f93d` | `a62cc384b9` | Incremental/full-tree equivalence test and workspace tests passed. |

The C4 task's second commit moved its allocation tests into a separate test file to match the repository code convention. No integration conflicts occurred in these merges. Phase 1 correctness is complete at `a4ae045cba2c86622bec755ec4011229608be499`: 270 core unit tests passed, two were ignored, all workspace tests passed, and Clippy exited successfully with warnings. The baseline workspace had 248 core unit tests passed and one ignored. Existing Clippy warnings were present before implementation; this gate did not treat warnings as errors.

B3's deterministic counter observes at most one prefix scan per input byte for malformed frames; no wall-time gain has been claimed from that structural test. B2's one release fanout observation used 2,000 records and 34,000 edges: fresh construction 19.385 ms, separate full relation validation 0.786 ms, and four reverse reconstructions instead of eight at the starting revision. One sample does not establish a speedup. The C6 canonical field rendering quotes additional punctuation; snapshots whose sort/configuration hash changes are invalidated and rebuilt through the ordinary compatibility path. Documentation must describe this compatibility effect with the final API update.

## Construction scale observations

B4 used a temporary release harness against synthetic schemas on the same 32-CPU machine. Each entry is one fresh-process sample, with identical registration order and field definitions before/after the change. At 4,000 temporal fields, registration took 1,105.366 ms before and 4.333 ms after incremental validation; measured peak process RSS was 10,972 KiB before and 7,828 KiB after. At 250 fields, the observed registration time was 7.485 ms before and 0.169 ms after. These timings isolate registration, not repository opening. The implementation preserves the full conversion-validation tree node by node under forward, reverse and mixed registration orders. One sample per side does not characterize variability.

The same focused review measured B4 shared-source reuse and R5 sparse mapping without changing either representation. A 512-file source shared by one versus four collections produced 512 versus 2,048 parses and 4.933 versus 9.272 ms fresh-build wall time. The file-to-record mapping occupied 4,227,584 bytes for 128 mostly disjoint collections with 64 files each (8,192 total files). Broader sharing and a sparse mapping are deferred pending workloads where those costs dominate; no speedup is claimed for them. Raw local harness results remain in the construction task's ignored `target/construction-results/` directory and are summarized here; no production behavior was changed for those two observations.
