# Typed query and relation checkpoint

Measured extended workload: `b70c7eef0512736bfdf5d550e9559e2b5e7c04f3`. The fixed historical 150-query run measured its code parent `2b118fa8c6e10f2fee4fcf5c69e5ffcd2ae80a10`; the intervening commit only changed four cases in the opt-in extended manifest. The earlier zero-match size and period fixtures remain in `../extended-2b118fa8c6/` as superseded raw evidence.

The extended manifest keeps Q001–Q150 unchanged and adds Q151–Q188 for inline sort, typed parameters, field comparisons, array positions, temporal periods and macros, byte sizes, regex and case modes, forward and reverse relations, multi-hop paths, selected relation FTS, and combined predicates. Only the extended Musenalm adapter loads the six schema-annotated relations. The historical workload remains relation-free.

## Correctness

- Full optimized-versus-reference acceptance: all 188 cases passed in ASCII and Unicode at `2b118fa8c6`, including fresh-process snapshot restore. After the four fixture changes, their focused acceptance passed in both alphabets with 46 size matches and 4,928 period matches. The final timed run had zero failures across 3,760 samples. Q001–Q150 retained their counts and page sizes in both configurations.
- Independent handwritten jq oracle over the original JSON: 300/300 complete membership sets passed at `b70c7eef05` (150 cases × two alphabets). The separate raw-JSON relation oracle passed 18/18 complete membership sets (nine cases × two alphabets).
- `cargo test --workspace --all-targets`: 235 passed, zero failed across 23 test binaries. The TUI suite includes structured record rendering, outgoing/backlink navigation, mouse click navigation and old-settings compatibility. `cargo clippy --workspace --all-targets` passed with advisory warnings. `cargo fmt --all -- --check` still reports existing repository-wide formatting differences, so no broad formatting rewrite was mixed into this feature work.

## Timing protocol

Both timed runs used the same copied Musenalm corpus, source/schema fingerprints, `rustc 1.99.1`, release build, 32 query workers, zero source/decoded caches, ASCII and Unicode, verified cache off and 64 MiB warm, and five timed repetitions. The benchmark reports per-request search time after repository opening; build and five fresh-process restore timings are separate. The full resolved query/options, min/median/max milliseconds, candidate/read/scan/parse counts, total matches and page sizes are joined in [FULL_QUERY_TABLE.csv](FULL_QUERY_TABLE.csv). Raw samples, plans, schema manifest, build timings and exact commands are under [timed/](timed/). Absolute differences below are sums of per-query medians, not one end-to-end batch latency.

| Profile | Fixed Q001–Q150, ms | Extended Q001–Q150, ms | Difference, ms | Added Q151–Q188, ms |
| --- | ---: | ---: | ---: | ---: |
| ASCII, cache off | 1,954.327 | 2,117.316 | +162.989 | 507.306 |
| ASCII, 64 MiB warm | 1.456 | 1.442 | -0.014 | 0.177 |
| Unicode, cache off | 1,846.941 | 2,008.975 | +162.034 | 503.146 |
| Unicode, 64 MiB warm | 1.489 | 1.417 | -0.072 | 0.186 |

The extended repository built six relation indexes and 228,503 edges, occupying an estimated 11.976 MiB. For the original 150 cases, it opened 18,386 fewer source files per uncached profile. Eleven cases changed source work, mostly typed ID/field paths; Q083 fell from 4,928 source opens to zero, for example. The complete per-case fixed-versus-extended comparison is [historical-subset-comparison.csv](historical-subset-comparison.csv). Broad scans such as Q006 and Q007 retained the same 148,486 source opens but were about 35 ms slower each in two extended recordings. Q117 was faster by about 45 ms. These timings are observations from separate process blocks, with no demonstrated change in scan algorithm; the extra relation-index footprint and machine/cache conditions have not been isolated as a cause. The warm profiles show no material slowdown.

| Alphabet | Fixed build, ms | Extended build, ms | Added, ms | Fixed restore median, ms | Extended restore median, ms | Fixed / extended snapshot, MiB | Fixed / extended peak RSS, MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ASCII | 1,036.521 | 1,197.233 | +160.712 | 459.906 | 502.514 | 106.062 / 111.791 | 556.500 / 623.887 |
| Unicode | 1,062.643 | 1,199.363 | +136.720 | 466.318 | 506.900 | 108.456 / 114.185 | 793.734 / 873.812 |

The increased build, snapshot, restore and memory cost belongs to the opt-in relation indexes. Peak RSS is a process high-water mark, not an allocation count. The fixed historical 150-query comparison with the controlled old-core recording is in [comparison-controlled-6b/COMPARISON.md](../query-extensions-final-2b118fa8c6/comparison-controlled-6b/COMPARISON.md): +110.399 ms ASCII uncached and +38.158 ms Unicode uncached across the 150 medians, with unchanged answer and source-work counts. Several broad queries varied substantially between independent runs, so these aggregate differences do not establish a per-query regression.

## Evidence

- [Historical fixed 150 recording](../query-extensions-final-2b118fa8c6/REPORT.md), including the complete timed table and matched protocol comparison.
- [Final extended timed recording](timed/REPORT.md), [independent jq oracle](jq-oracle/REPORT.md), and [independent relation oracle](relation-oracle/REPORT.md).
- [Workspace test log](workspace-tests.log), [Clippy log](clippy.log), [ASCII correction acceptance](accept-corrected-ascii.log), and [Unicode correction acceptance](accept-corrected-unicode.log).
