# Registered text recovery: focused comparison

Candidate source: `8607244b4123` on `library-extensions-stage2-wip`; production baseline: `266d9a0fb071838be8f757671599872a7036a054`. The exact executable digests, corpus fingerprint, commands, raw-output digests and validation exits are in [evidence-manifest.json](evidence-manifest.json). The candidate release executable was built from this production source before the later documentation and test-only edits in the same commit. The reference executable is the pinned Phase 0 binary.

## Default-path FTS comparison

The existing 30-case `fts-query-mix` ran with the same temporary Musenalm schema overlay, five alternating reference/candidate process pairs per cache budget, seven measured query rounds per process, 16 Rayon workers and CPU affinity 0–15. One full process per binary and budget warmed the filesystem and indexes. The same corpus content, source timestamps and configuration matched the saved Phase 0 fingerprint before timing. Each measured process returned 30 complete case rows; match counts, first IDs, first fields and paths agreed in every pair. Query-cache budgets 0 and 64 MiB were kept separate. The runner checks signatures and saves every raw sample and stderr file.

The reference and candidate each produced 12,823,195 base index bytes and 34,632,979 FTS index bytes. Cache-off construction medians were 522.22 ms and 523.93 ms; cache-on construction medians were 522.38 ms and 522.07 ms. Peak RSS after construction was about 210–211 MiB on both. These normal release runs did not use counting allocators or detailed stage timers.

Per-case process medians and paired-delta ranges follow. Positive deltas mean candidate slower; `summary.json` also contains every paired delta and candidate count. Candidate counts were identical in all measured pairs.

| Cache MiB | Case | Reference ms | Candidate ms | Delta ms | Paired delta range ms | Candidates |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 1 | 1.806 | 1.813 | +0.007 | -0.054…+0.025 | 425 |
| 0 | 2 | 5.040 | 5.048 | +0.008 | -0.032…+0.054 | 3227 |
| 0 | 3 | 2.625 | 2.563 | -0.062 | -0.097…+0.019 | 1087 |
| 0 | 4 | 1.794 | 1.728 | -0.066 | -0.126…-0.010 | 425 |
| 0 | 5 | 3.300 | 3.340 | +0.041 | -0.082…+0.061 | 1800 |
| 0 | 6 | 0.934 | 0.982 | +0.048 | -0.165…+0.189 | 425 |
| 0 | 7 | 3.306 | 3.226 | -0.080 | -0.130…+0.016 | 1064 |
| 0 | 8 | 2.800 | 2.669 | -0.131 | -0.212…+0.040 | 849 |
| 0 | 9 | 110.090 | 112.633 | +2.543 | +2.283…+3.513 | 0 |
| 0 | 10 | 107.289 | 109.312 | +2.023 | +1.653…+3.128 | 0 |
| 0 | 11 | 1.596 | 1.601 | +0.005 | -0.114…+0.131 | 61 |
| 0 | 12 | 2.009 | 2.012 | +0.002 | -0.077…+0.035 | 501 |
| 0 | 13 | 1.341 | 1.287 | -0.053 | -0.093…-0.022 | 61 |
| 0 | 14 | 3.012 | 3.005 | -0.007 | -0.068…+0.036 | 2144 |
| 0 | 15 | 4.841 | 4.789 | -0.052 | -0.158…+0.012 | 3582 |
| 0 | 16 | 2.246 | 2.239 | -0.006 | -0.048…+0.062 | 687 |
| 0 | 17 | 2.875 | 2.922 | +0.047 | +0.024…+0.079 | 1368 |
| 0 | 18 | 0.953 | 0.922 | -0.031 | -0.045…-0.007 | 0 |
| 0 | 19 | 1.612 | 1.597 | -0.015 | -0.034…-0.006 | 325 |
| 0 | 20 | 1.035 | 1.018 | -0.017 | -0.119…+0.192 | 0 |
| 0 | 21 | 1.360 | 1.473 | +0.113 | -0.083…+0.258 | 325 |
| 0 | 22 | 3.650 | 3.567 | -0.083 | -0.244…+0.178 | 1341 |
| 0 | 23 | 0.538 | 0.532 | -0.006 | -0.010…+0.003 | 61 |
| 0 | 24 | 6.848 | 6.901 | +0.052 | -0.014…+0.068 | 3651 |
| 0 | 25 | 5.781 | 5.247 | -0.534 | -0.573…-0.481 | 500 |
| 0 | 26 | 1.156 | 1.156 | +0.000 | -0.014…+0.026 | 341 |
| 0 | 27 | 1.439 | 1.426 | -0.013 | -0.036…+0.021 | 662 |
| 0 | 28 | 0.385 | 0.394 | +0.009 | +0.002…+0.012 | 39 |
| 0 | 29 | 176.098 | 177.522 | +1.424 | +0.063…+2.073 | 133948 |
| 0 | 30 | 25.819 | 25.946 | +0.128 | -0.256…+0.278 | 13542 |
| 64 | 1 | 0.060 | 0.059 | -0.001 | -0.004…+0.001 | 0 |
| 64 | 2 | 0.059 | 0.058 | -0.001 | -0.002…+0.001 | 0 |
| 64 | 3 | 0.055 | 0.054 | -0.000 | -0.001…+0.002 | 0 |
| 64 | 4 | 0.065 | 0.063 | -0.001 | -0.003…+0.000 | 0 |
| 64 | 5 | 0.056 | 0.055 | -0.001 | -0.003…+0.000 | 0 |
| 64 | 6 | 0.479 | 0.485 | +0.006 | -0.000…+0.016 | 0 |
| 64 | 7 | 0.473 | 0.479 | +0.006 | -0.002…+0.012 | 0 |
| 64 | 8 | 0.494 | 0.490 | -0.004 | -0.011…+0.050 | 0 |
| 64 | 9 | 0.074 | 0.075 | +0.001 | -0.002…+0.004 | 0 |
| 64 | 10 | 0.064 | 0.064 | -0.000 | -0.002…+0.002 | 0 |
| 64 | 11 | 0.059 | 0.059 | -0.000 | -0.002…+0.002 | 0 |
| 64 | 12 | 0.059 | 0.060 | +0.001 | -0.003…+0.002 | 0 |
| 64 | 13 | 0.054 | 0.055 | +0.000 | -0.002…+0.002 | 0 |
| 64 | 14 | 0.056 | 0.058 | +0.001 | -0.000…+0.002 | 0 |
| 64 | 15 | 0.025 | 0.026 | +0.000 | -0.000…+0.001 | 0 |
| 64 | 16 | 0.057 | 0.057 | +0.000 | -0.000…+0.003 | 0 |
| 64 | 17 | 0.058 | 0.055 | -0.003 | -0.010…+0.003 | 0 |
| 64 | 18 | 0.027 | 0.027 | +0.000 | -0.001…+0.002 | 0 |
| 64 | 19 | 0.025 | 0.026 | +0.001 | -0.000…+0.002 | 0 |
| 64 | 20 | 0.026 | 0.027 | +0.001 | -0.002…+0.003 | 0 |
| 64 | 21 | 0.215 | 0.218 | +0.003 | -0.005…+0.005 | 0 |
| 64 | 22 | 0.240 | 0.248 | +0.008 | +0.003…+0.010 | 0 |
| 64 | 23 | 0.197 | 0.199 | +0.002 | -0.000…+0.004 | 0 |
| 64 | 24 | 0.521 | 0.529 | +0.009 | -0.000…+0.026 | 0 |
| 64 | 25 | 4.061 | 3.498 | -0.563 | -0.619…-0.470 | 0 |
| 64 | 26 | 0.382 | 0.385 | +0.003 | +0.002…+0.015 | 0 |
| 64 | 27 | 0.288 | 0.290 | +0.001 | -0.005…+0.012 | 0 |
| 64 | 28 | 0.192 | 0.191 | -0.001 | -0.005…+0.005 | 0 |
| 64 | 29 | 0.149 | 0.150 | +0.001 | -0.002…+0.006 | 0 |
| 64 | 30 | 4.253 | 4.384 | +0.131 | -0.010…+0.363 | 0 |

The largest candidate median increase was case 9 at +2.54 ms (110.09 to 112.63 ms), followed by case 10 at +2.02 ms (107.29 to 109.31 ms). Both are JSON-filter cases outside the new registered scanner path. The all-record scan case 29 rose by +1.42 ms (176.10 to 177.52 ms), with the same 133,948 candidates. No 80–100 ms slowdown appeared in this matrix. Small timing deltas remain visible in the raw data and are not relabeled as speedups.

## Native versus registered built-in search

The downstream [text_search_compare.rs](../../../../extension-tests/examples/text_search_compare.rs) example applied the same temporary schema overlay as `fts-query-mix`, registered plain/Markdown/HTML/XML representations through public core APIs, and compared native and registered searches in alternating order for seven rounds. It then collected complete ordered IDs, fields, concrete paths, totals and truncation outside timing. All eight complete signatures matched; their hashes are in [registered-signatures.json](registered-signatures.json).

| Cache MiB | Field | Matches | Candidates | Native median ms | Registered median ms | Delta ms |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 0 | `annotation` | 284 | 325 | 1.119 | 1.238 | +0.118 |
| 0 | `preferred_title` | 59 | 61 | 0.490 | 0.502 | +0.013 |
| 0 | `title_statement` | 643 | 687 | 1.713 | 1.730 | +0.017 |
| 0 | `contributors[].annotation` | 0 | 500 | 1.203 | 1.210 | +0.007 |
| 64 | `annotation` | 284 | 0 | 0.175 | 0.190 | +0.016 |
| 64 | `preferred_title` | 59 | 0 | 0.170 | 0.182 | +0.012 |
| 64 | `title_statement` | 643 | 0 | 0.232 | 0.236 | +0.004 |
| 64 | `contributors[].annotation` | 0 | 0 | 0.395 | 0.404 | +0.010 |

With cache disabled, native and registered paths performed the same candidate scans and parses: 325/325 for Markdown, 61/61 for plain text, 687/687 for HTML and 500/500 for the XML field. With 64 MiB cache, both paths performed zero candidate scans; positive-result pages still parsed one returned record. The Musenalm XML field has no matching `Goethe` record; the separate external fixture verifies a positive XML inline-tag match. The largest registered median increase was 0.12 ms in the cache-off Markdown case. These timings are a focused comparison, not a claim of zero overhead for all extensions.

The first two `registered-cache*.ndjson` files were run without the FTS benchmark’s temporary schema overlay. They returned zero registered Markdown/HTML matches because the stored Musenalm schema declares those fields plain. They are retained as a setup diagnostic; the corrected `registered-overlay-cache*.ndjson` files are the accepted comparison.

## Validation and limits

Workspace tests, workspace Clippy, core tests with `index-timings`, and benchmark Clippy with both `index-timings` and `allocation-profile` passed. The repository-wide spacing check still fails on existing unchanged files, including `bigram/filter.rs` and `config/discovery.rs`; none of the changed Rust files appeared in its output. Its saved stdout has trailing whitespace removed for diff hygiene; the formatting differences themselves are retained. `git diff --check` passed. The targeted external test covers snapshot reuse and an edited HTML source in addition to candidate parity. The previous Gate A [resolution](../20261104T144149Z/RESOLUTION.md) records the instrumented build uncertainty and the production-like FTS4 comparison.

This is a focused Gate B/C text-stage result. The final mixed JSON/cache/sort/build/restore matrix, allocation-profile comparisons for this stage, and the original plan’s typed extractor, custom provider/decoder, query frontend and module-separation stages remain open. The correction to the initial external benchmark setup does not change production source semantics.
