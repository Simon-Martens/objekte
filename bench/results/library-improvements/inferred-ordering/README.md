# Inferred posting order reuse

Measured 2026-11-07 on Linux x86_64 (32 available CPUs), `rustc 1.99.1` and `cargo 1.99.1`, release profile. The code revisions were the exact pre-Q3 base `0b0ea28f8f901680402fd316d5c97d80ea51eee9` and Q3 implementation `fc9beb2edf4c60a3836e639963ddde82de85d24f`. The baseline process completed before the final process; no affinity or cache dropping was used.

The same public-API harness source was copied into both worktrees. `Cargo.toml` SHA-256: `2d49c8a249cdeaba367f21096aa79aa3e82be1d325b0a585f73ad95447d3e362`; `src/main.rs` SHA-256: `18d05527314c6879d7e19eb9ac5e13bebd564f61c11568d5bf77875bdb716ea9`; generated `Cargo.lock` SHA-256: `06aa4955eae6dd26cbe4ac4eb55ec4722c4003dfbf0bf6bf824b7748c78a72a7`. The deterministic JSONL fixture has 4,096 records, 258,048 bytes, and SHA-256 `1aba864447ae0184f0822bb39ac69af84a95bcd044dc104a10c345cb812aba74` in both worktrees.

The harness uses AutoIndex with a larger distinct-string allowance for its UUID-like field, ASCII bigrams, snapshot mode disabled, verified cache disabled, one query worker and one build worker. It opens a fresh repository for each of five blocks, warms query binding through estimate-only `explain`, then times one first sorted page and ten repeats of the same page. The query sorts `uuid` ascending, offset 100, limit 20, locator return, count disabled. Repository opening, fixture generation and preparation are outside the timed interval. Every timed request returned 20 records with zero file opens and zero record parses.

Commands in each respective worktree, with `RUN` set to `fixture` for baseline and `final-fixture` for Q3:

```sh
cargo build --release --manifest-path bench/results/library-improvements/inferred-ordering/harness/Cargo.toml
mkdir -p bench/results/library-improvements/inferred-ordering/$RUN
bench/results/library-improvements/inferred-ordering/harness/target/release/objekte-q3-timing bench/results/library-improvements/inferred-ordering/$RUN
```

| Request | Pre-Q3 median ms | Q3 median ms | Difference ms | Observed change | Q3 range ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| First page, 5 samples | 4.245 | 3.850 | -0.395 | -9.3% | 3.793–5.358 |
| Reuse, 50 samples | 4.173 | 0.091 | -4.082 | -97.8% | 0.051–0.098 |

The first-use difference is inconclusive given its variability and this small sequential run. The repeated-page difference follows the structural change: the cache builds an order once per field, generation, locale, collation version, options and direction, then reuses the same immutable `SortColumn`. Unit counters verify one build and subsequent hits; eviction tests verify its 128 MiB retained-byte estimate. The existing dictionary already computes one ICU key per distinct string, although the `SortColumn` representation still duplicates keys per record. A dictionary cursor would need a separate paging/comparator route for ICU strings, missing/null placement and stable ID ties, so this experiment retained the existing verified ordering path. Evicted entries may rebuild, and in-flight `Arc` references can temporarily outlive an eviction. The cache is not persisted; fresh build and snapshot restore both start empty.

Raw samples: [`baseline-0b0ea28.csv`](baseline-0b0ea28.csv), [`final-fc9beb2.csv`](final-fc9beb2.csv). Release compiler output: [`baseline-build.stderr.log`](baseline-build.stderr.log), [`final-build.stderr.log`](final-build.stderr.log).

## Cold-build lock follow-up

The original Q3 implementation held the cache mutex while constructing a missing `SortColumn`. Commit `81db11a03a16339b4141713c438690b6e97bdf07` moves construction between a locked lookup and locked insert-if-absent. Two builders for the same key may do duplicate construction, but only one column is retained and charged. Controlled-gate tests verify that distinct-field builds can overlap and same-key builds publish one entry. The full workspace suite passes (core: 280 passed, 2 ignored), as does Clippy; no source or snapshot format changed.

The unchanged harness was rebuilt in release mode and rerun on the same fixture and machine in a separate uncontended window. The fixture SHA-256 remains `1aba864447ae0184f0822bb39ac69af84a95bcd044dc104a10c345cb812aba74`. The new raw sample is [`final-81db11a.csv`](final-81db11a.csv), measured at exact code SHA `81db11a03a16339b4141713c438690b6e97bdf07`. Compiler output is [`final-81db11a-build.stderr.log`](final-81db11a-build.stderr.log).

| Request | Pre-Q3 `0b0ea28` median ms | Original Q3 `fc9beb2` median ms | Cold-lock fix `81db11a` median ms | Cold-lock range ms |
| --- | ---: | ---: | ---: | ---: |
| First page, 5 samples | 4.245 | 3.850 | 4.052 | 4.038–4.079 |
| Reuse, 50 samples | 4.173 | 0.091 | 0.092 | 0.048–0.095 |

Reuse is still 4.082 ms faster per request than the pre-Q3 baseline (about 97.8%). The cold-lock change targets concurrent misses; this single-thread harness cannot measure that benefit. It shows no material reuse change versus the original Q3 code. All 55 new requests returned 20 rows and opened/parsed zero source files.
