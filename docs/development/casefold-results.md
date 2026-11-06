# Shared case-folded bigram results

Measured code parent: `ff759b225c6bc4bfdd614c542fe38fe017a900b6` on `perf/core-shared-casefold-bigrams`. The starting development revision was `85120a38beaf94de03519f6b2e6c1d05067406db`, including the foundation repair. The comparable before-state is the repaired core at `20261604bebf949131441c8262f1f442bce4207c`, not the older foundation recording. Each implementation and test step was committed and pushed separately. This report and its artifacts are the final documentation step; the measured core did not change afterward.

## Correctness and representation

**Correctness passed.** The fixed 150-case acceptance runner compared complete optimized and independent reference results, including counts, ordered pages, evidence, status, diagnostics, and payload, for both alphabets and restored snapshots. The independent handwritten jq oracle matched all **300/300 complete ID sets** over 148,486 original files. Workspace tests passed: 94 core, 25 TUI, and two example/CLI tests. Formatting and normal workspace Clippy completed; Clippy reported 50 core library warnings and two benchmark-example warnings. The [acceptance logs](../../bench/results/musenalm/casefold-full-ff759b225c/), [jq report](../../bench/results/musenalm/casefold-jq-oracle-ff759b225c/REPORT.md), and [test log](../../bench/results/musenalm/casefold-full-ff759b225c/workspace-tests.log) retain the evidence.

The index now stores normal and skip grams from full, non-Turkic Unicode case folding using the pinned `unicode-casefold` 0.2.0 / Unicode 9.0 tables. Both sensitive and insensitive requests prepare the same folded necessary grams. Exact sensitive verification retains the original literal and text; exact insensitive verification folds both. ASCII retains printable-byte windows over folded UTF-8; Unicode uses folded scalar windows. Tests cover expansion (`ß`→`ss`), sigma, dotted/dotless I, decomposed accents, unsupported ASCII bytes, separate segments, short literals, fallback records, aliases, caches, and restore. A custom exact text matcher without an insensitive-mode contract is rejected during preparation.

Snapshot compatibility fingerprints the folded policy and `BIGRAM_VERSION = 3`. Old representations are rejected and rebuilt, including a test of the normal configuration-mismatch route. Query-mode switching uses the same loaded generation. In the Musenalm configuration, the index has **15 physical bigram stores** per alphabet, with **25,477 ASCII** or **37,909 Unicode** distinct normal/skip keys. The paired-mode runner checked that stores and keys stayed unchanged through alternating requests. A separate fixture confirmed that identity and built-in folded aliases over the same field create one custom store.

## Comparable measurement conditions

The [full current run](../../bench/results/musenalm/casefold-full-ff759b225c/) and [build run](../../bench/results/musenalm/casefold-build-ff759b225c/) use the recorded repaired-core baseline under the same i9-13900K, Rust 1.99.1 release, performance governor, affinity 0–31, 32 query workers, zero source/decoded-cache budgets, and unchanged Musenalm schema/corpus. The corpus fingerprint is `9f93a4ba7fb9249c9c4eb04e03ed911435a6b0493f15b087a5447add32456aa9`. The full query run kept all 150 existing requests, both alphabets, verified cache off or warm at 64 MiB, and five timed repetitions per case/profile. Count, page, and payload work stayed inside query timing; diagnostic plans were collected separately. The full 600-row [timing table](../../bench/results/musenalm/casefold-full-ff759b225c/TIMING_TABLE.md) prints each resolved query/options, absolute milliseconds, candidates, source opens, scans, parses, matches, and page size. Raw samples, plans, manifests, and the printed gate output are beside it.

The fixed workload's answers did not change. Its timing gate remains **red: 13 per-case flags** in 3,000 valid samples, although the aggregate median-sum ratio was 1.017. The five flagged cases with larger folded candidate domains are the expected sensitive selectivity tradeoff. The eight flags with unchanged candidate and source-work counts were investigated in alternating old/new process blocks. Their large full-run differences did not persist as a coherent >2 ms regression in the focused repeats. The raw gate failures remain recorded; no threshold was changed.

| Profile | Repaired baseline ms | Folded index ms | Difference ms | Difference % |
| --- | ---: | ---: | ---: | ---: |
| ASCII, verified cache off | 2,054.344 | 2,019.855 | -34.489 | -1.7% |
| Unicode, verified cache off | 1,871.646 | 1,973.963 | +102.317 | +5.5% |
| ASCII, warm verified cache | 1.841 | 1.665 | -0.176 | -9.5% |
| Unicode, warm verified cache | 2.680 | 2.574 | -0.106 | -4.0% |

These sums add 150 per-query medians; they are not a single workload wall time. Warm-cache absolute differences are below 0.2 ms across the whole profile and should not be read as a meaningful percentage speedup.

The clearest sensitive losses are Q026 (`contents` FTS `Der`) and Q078 (`incipit_statement ~ "Der"`) in Unicode. Both grow from **19,419 to 81,159 candidates/source opens**, with identical answers. Full-run medians were **43.201→131.686 ms** and **18.826→72.918 ms**, respectively. In alternating A–B–B–A process blocks of 11 repetitions, the corresponding median differences were **+96.310 ms** and **+45.521 ms**. The added source verification follows directly from folding `Der` and `der` into the same grams. Q001 and Q014 likewise grow 3,424→4,872 and 1,744→2,896 candidates; Q100 grows 18,227→22,570. The design deliberately retains sensitive exact rejection after these broader candidates. No second sensitive index was added.

Unchanged-work controls were checked in the [first paired blocks](../../bench/results/musenalm/casefold-paired-ff759b225c/), a [31-repetition Q113/Q114 repeat](../../bench/results/musenalm/casefold-unchanged-work-paired-ff759b225c/), and [31-repetition repeats for the other unchanged-work flags](../../bench/results/musenalm/casefold-unaffected-paired-ff759b225c/). For example, Q114 Unicode has 4,928 candidates, opens, and parses in both versions; its first paired blocks were 3.221/3.305 ms before versus 7.146/6.142 ms after, but the longer repeat gave block medians 4.478/4.176 ms before versus 4.688/4.438 ms after, with substantially overlapping samples. Q056 Unicode repeated at 0.893→0.940 ms, Q107 at 12.852→12.834 ms, and Q099 ASCII at 3.314→2.705 ms. Small residual variation remains, but the multi-millisecond full-run flags cannot be attributed to additional candidate or source work.

The existing insensitive FILTER workload demonstrates the intended gain. Q067 `name:lower ~ "goethe"` changed from **4,010 to 1 candidate/open** and **3.579→0.121 ms** ASCII (3.355→0.128 ms Unicode). Q077 `preferred_title:lower ~ "FRÜHLING"` changed from **133,948 to 4,636 candidates** and **99.610→5.188 ms** ASCII; Unicode changed to **2,011 candidates** and **103.967→2.471 ms**. Q063 similarly fell from 210 to one candidate. Named `:folded` Q070 retained its one-candidate path.

## New per-request FTS case mode

The [paired-mode table](../../bench/results/musenalm/casefold-full-ff759b225c/PAIRED_MODE_TABLE.md) and [query JSON](../../bench/results/musenalm/casefold-full-ff759b225c/paired-modes-queries.json) are first measurements for the new insensitive FTS option; no historical unscoped equivalent is asserted. They alternate modes on the **same loaded repository**, with verified cache off for primary samples and 64 MiB warm in a separate profile. Plans are outside timed samples. Both modes get identical folded candidate domains for a given literal; their different match counts come from exact verification.

| Alphabet | Unscoped FTS | Sensitive ms / matches | Insensitive ms / matches | Shared candidates |
| --- | --- | ---: | ---: | ---: |
| ASCII | `Der` | 112.836 / 19,681 | 178.498 / 80,871 | 89,586 |
| Unicode | `Der` | 110.267 / 19,681 | 184.098 / 80,871 | 89,753 |
| ASCII | `Müller` | 32.466 / 1,164 | 48.741 / 1,221 | 15,219 |
| Unicode | `Müller` | 5.002 / 1,164 | 8.205 / 1,221 | 1,257 |
| Unicode | `ß` | 88.967 / 33,078 | 124.516 / 55,544 | 55,544 |

Insensitive exact verification folds scanned text and may cost more per candidate; the paired data makes that cost visible rather than mixing it into warm-cache totals. Selected `name` FTS for `Goethe` uses one candidate/open in both modes, selected `preferred_title:lower` uses 20 Unicode candidates, and the combined FTS/FILTER query retains its FILTER semantics. FILTER-only controls are labeled with a mode in the table to show that changing the FTS option has no effect when FTS is absent. A small mixed-case fixture tests exact variants independently of corpus frequencies.

## Build, restore, and memory

Each fresh build configuration ran three independent processes under the same policy. All parsed 148,486 records with zero failed parses. The [build report and raw samples](../../bench/results/musenalm/casefold-build-ff759b225c/) contain discovery, extraction/indexing, finalization, snapshot write, RSS, heap, gram/store, and parser counts.

| Configuration | Repaired baseline wall s | Folded wall s | Difference s | Folded peak RSS MiB |
| --- | ---: | ---: | ---: | ---: |
| ASCII, snapshots disabled | 11.343 | 11.958 | +0.615 | 236.5 |
| ASCII, snapshot rebuild | 11.842 | 12.260 | +0.418 | 339.4 |
| Unicode, snapshots disabled | 12.089 | 12.165 | +0.076 | 242.2 |
| Unicode, snapshot rebuild | 12.647 | 12.401 | -0.246 | 346.9 |

The ASCII build increase is about half a second in this three-run set; no construction speedup is claimed. Unicode snapshot size fell **122,383,021→113,724,609 bytes** (−8,658,412 bytes, −7.1%) because folded keys merge case variants. ASCII rose only 37,204 bytes, to 111,214,033. Five fresh-process restore samples in the full query runner had medians **494.560→486.991 ms** ASCII and **512.665→504.034 ms** Unicode. The raw phase and memory samples remain available for later comparison.

The representation intentionally exchanges Unicode sensitive selectivity for one shared index and efficient case switching. The independent correctness checks are green; the full per-query timing gate is red, with the repeatable large losses explained by candidate growth and the unchanged-work flags documented separately.
