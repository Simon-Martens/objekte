# Borrowed segment verification results

Task: `borrowed-segment-verification` · 11 November 2026
Predecessor and task base: `5166876c7228f4fe2f8f064861cdc8a88275ad18`
Implementation: `1f906cb1be`, `c3b01ab22d`, `47931f0cdf`
Branch: `integration/borrowed-segment-verification`
Detailed measurements: [benchmark results](../../../bench/results/borrowed-segment-verification/README.md) and [40-row focused comparison](../../../bench/results/borrowed-segment-verification/focused-comparison.md).

## Disposition

| Requirement | Result |
| --- | --- |
| A, existing `Value` | Built-in Json/JsonLines local FTS now visits `Value::String` and `Value::Id` by `&str`, with borrowed object keys in a reusable path stack. Query projections stop producing owned text segments for that route. The existing `Value` remains the owner through the match. Relations and logical output use the same borrowed visitor where the value supplies the searched text. A pointer test checks text/key identity; test-only copied-byte counters show no query-time collector copy. |
| B, source bytes | Eligible literal FTS, one local contains FILTER, and their conjunction scan Json, JsonLines, or JsonDecodedScalars source bytes without constructing a matching-only logical tree or `TextSegment`. Plain strings are borrowed directly; escaped strings decode into worker scratch. Decoded-scalar number/bool/null tokens are borrowed as lexemes. The source visitor keeps provisional match state through duplicate resolution, so unscoped decoded FTS sees all occurrences and field-scoped matching sees only the final member. |
| C, reusable storage | Full Unicode folding now writes into a worker-local `String` through `fold_into`; decoded source strings use another worker-local `String`. The two buffers remain live separately while escaped text is folded. Provisional events reuse a worker-local vector. Each of the three retained buffers has a 256 KiB post-record bound; total is at most 768 KiB per worker, plus path and evidence allocations. Oversized buffers are released after the record without limiting valid record size. |

The direct plan is prepared once per collection query. It excludes regex, opaque bindings, complex Boolean or field-to-field FILTER, relations, explicit array selectors, computed sort, and incompatible readers. Json/JsonLines FTS with schema conversions uses the value route. Static logical output uses its required value/combined parse, avoiding a second full parse for high-hit pages. JsonDecodedScalars value-required FTS keeps the original source-derived corpus, including overwritten duplicate occurrences and numeric spellings. Owned `TextSegment`, `ExtractedRecord`, and `Value` APIs remain for index construction, public extraction, extensions, and fallback evaluators. Regex uses its existing matcher; custom opaque transforms retain their owned fallback.

## Correctness and validation

The focused tests cover crossing a decoded escape within one segment, separation across fields and array elements, long scalars, Unicode fold, wildcard and escaped-percent contains, empty patterns, missing/null/non-string values, empty and mixed arrays, duplicate replacement in both directions, evidence after replacement, and unchanged source bytes. Integrated tests compare full hits against reference execution across Json, JsonLines, and JsonDecodedScalars with evidence on/off and locator, original-byte, and logical output. They also exercise combined FTS/FILTER, pagination, and the value-required fallback. Existing workspace tests cover ordering, streaming, cancellation, extension/regex behavior, and the explicit validation policy. No search freshness probe or index mutation was added.

`cargo test --workspace` passed on the final source (486 core tests passed, 12 ignored; 158 desktop tests passed, one ignored). A preceding run had one timing-sensitive desktop scheduler failure; its isolated retry and the final full run passed. `cargo clippy --workspace --all-targets` passed with existing workspace warnings. Task-owned files pass the repository spacing check and `git diff --check`. The repository-wide spacing check still reports an unrelated existing desktop formatting issue at `crates/desktop/src/ui/workspace.rs:581`.

## Performance

The identical benchmark example ran serially on the exact predecessor and final revisions under Rust/Cargo 1.99.1 release builds, four workers, a disabled verified-result cache, and warm OS caches. Focused searches had seven timed repetitions after warm-up and a separate allocation-counted pass. All 40 focused base/final comparisons agree on membership SHA-256, counts, candidates, source bytes/opens, and parse counts. The five direct-source shapes per fixture used B for every candidate; two value-required shapes and high-hit logical output used their required value/combined route. Representative complete-query results (median milliseconds; gross untimed allocation count):

| Fixture and query | Candidates / hits | Base → final ms | Base → final allocations |
| --- | ---: | ---: | ---: |
| Plain Json, value identity | 1,365 / 409 | 1.242 → 1.547 | 54,652 → 38,272 |
| Plain Json, value folded | 1,365 / 409 | 1.259 → 1.255 | 54,652 → 38,272 |
| Plain Json, source identity | 1,365 / 1,365 | 1.570 → 1.494 | 54,880 → 34,405 |
| Escaped decoded JSON, combined | 1,365 / 1,365 | 1.761 → 1.491 | 80,839 → 30,334 |
| Field duplicates plus 128 scalars, scoped FTS | 342 / 171 | 3.599 → 0.513 | 287,225 → 6,101 |
| Plain Json, high-hit logical output | 1,365 / 1,365 | 3.179 → 2.981 | 55,232 → 38,852 |
| Long escaped decoded JSON, combined | 683 / 683 | 6.529 → 7.409 | 45,946 → 15,211 |

The table's allocation counts include file buffers, output, evidence, and duplicate bookkeeping. At the matching boundary, tests verify zero copied `Value` string bytes for A identity and zero owned-segment text bytes for B. Unescaped identity B borrows a source slice; escaped B writes decoded bytes once to scratch; folding writes its output once to separate scratch. Warmed scratch capacity and pointer tests show no further growth for alternating ordinary segments/records. The detailed comparison gives gross allocated bytes, source bytes read, and min/max dispersion for every case. The public in-memory owned extraction control kept identical allocation counts and bytes on baseline and final, as expected for a retained API; its plain trusted `fts_good` median was 1.846 → 1.807 ms and escaped trusted `fts_good` 2.366 → 2.333 ms.

The full 194-query Musenalm suite passed in ASCII/Unicode with cache off and 64 MiB warm cache. Across 776 case/profile rows, result and work counters match exactly. Median per-case final/base timing ratios were 0.975 and 0.983 with cache off, 1.010 and 1.008 warm. A shared build routine changed, so builds were compared: both parsed 148,486 records per alphabet and produced equal estimated index sizes. Across two suite runs, ASCII base/final build times were 1.578/1.243 and 1.268/1.198 seconds; Unicode times were 1.374/1.280 and 1.285/1.289 seconds.

## Limitation

The safe reusable decoder remains slower on synthetic 8 KiB escape-heavy strings for some direct-source shapes. Three serial seven-repetition comparisons gave final/base median ratios of 1.135, 1.049, and 1.060 for combined FTS/contains, and 1.101, 1.056, and 1.067 for field-scoped FTS. Fusing quote location with decoding and tracking only relevant duplicate keys reduced the initial slowdown. The remaining cost is escape decoding into reusable storage versus the predecessor's allocating decoder. Returning these cases to per-segment owned extraction would remove B's allocation reduction. High-hit logical output follows the prior combined path; its long-string ratios varied 1.141, 0.938, and 1.048, consistent with run-to-run timing variation. No result or work-counter regression was observed.
