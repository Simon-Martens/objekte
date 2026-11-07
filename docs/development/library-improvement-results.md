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
| Q1 access-path planning | `407a55ed23` | `267946c29a` | Three focused estimate/reuse tests, workspace tests and Clippy passed. |
| Q2 estimate-only EXPLAIN | `a556422932` | `0b0ea28f8f` | Relation/related-FTS no-read test, workspace tests and Clippy passed. |
| B1 bounded inference summaries | `debf099560` | `de8ad6a9d4` | Manual/opt-out partition and worker tests, workspace tests and Clippy passed. |
| Q3 inferred-order cache | `ee7f69657e` | `a7d1fb4573` | Three cache reuse/key/eviction tests, workspace tests and Clippy passed. |
| Q6 prepared membership evaluation | `50760c9d0a` | `d54038553f` | Wildcard and relation/FTS membership tests, workspace tests and Clippy passed. |

The C4 task's second commit moved its allocation tests into a separate test file to match the repository code convention. No integration conflicts occurred in these merges. Phase 1 correctness is complete at `a4ae045cba2c86622bec755ec4011229608be499`: 270 core unit tests passed, two were ignored, all workspace tests passed, and Clippy exited successfully with warnings. The baseline workspace had 248 core unit tests passed and one ignored. Existing Clippy warnings were present before implementation; this gate did not treat warnings as errors.

B3's deterministic counter observes at most one prefix scan per input byte for malformed frames; no wall-time gain has been claimed from that structural test. B2's one release fanout observation used 2,000 records and 34,000 edges: fresh construction 19.385 ms, separate full relation validation 0.786 ms, and four reverse reconstructions instead of eight at the starting revision. One sample does not establish a speedup. The C6 canonical field rendering quotes additional punctuation; snapshots whose sort/configuration hash changes are invalidated and rebuilt through the ordinary compatibility path. Documentation must describe this compatibility effect with the final API update.

## Construction scale observations

B4 used a temporary release harness against synthetic schemas on the same 32-CPU machine. Each entry is one fresh-process sample, with identical registration order and field definitions before/after the change. At 4,000 temporal fields, registration took 1,105.366 ms before and 4.333 ms after incremental validation; measured peak process RSS was 10,972 KiB before and 7,828 KiB after. At 250 fields, the observed registration time was 7.485 ms before and 0.169 ms after. These timings isolate registration, not repository opening. The implementation preserves the full conversion-validation tree node by node under forward, reverse and mixed registration orders. One sample per side does not characterize variability.

The same focused review measured B4 shared-source reuse and R5 sparse mapping without changing either representation. A 512-file source shared by one versus four collections produced 512 versus 2,048 parses and 4.933 versus 9.272 ms fresh-build wall time. The file-to-record mapping occupied 4,227,584 bytes for 128 mostly disjoint collections with 64 files each (8,192 total files). Broader sharing and a sparse mapping are deferred pending workloads where those costs dominate; no speedup is claimed for them. The [versioned construction evidence](../../bench/results/library-improvements/construction-scaling/README.md) includes the raw JSON, fixture source, exact commands, and settings. No production behavior was changed for those two observations.

## Query-planning scale observation

Q1 estimates inferred postings and temporal ranges without constructing their candidate bitmaps, and it reuses a filter plan when a selective stored-order attempt falls back to top-k execution. The focused 1/4/16-conjunct tests verify one estimate and one executed candidate construction per inferred term, bounded temporal dictionary probes, and exact result parity. The combined integration tests and Clippy passed after the no-ff merge.

The [matched release timing and raw samples](../../bench/results/library-improvements/query-planning/README.md) compare `a4ae045cba` with Q1 code commit `dfeb1123ee` on the same 8,192-record, 16-field fixture. Five blocks of 200 searches used one loaded repository per revision. Median query times were 0.121560→0.115416 ms for one conjunct (−0.006144 ms, −5.1%), 0.191679→0.139473 ms for four (−0.052206 ms, −27.2%), and 0.720725→0.219449 ms for sixteen (−0.501276 ms, −69.6%). These measurements show the intended planning improvement on this fixture; they do not quantify the full Musenalm workload or mixed access paths.

## First integration checkpoint

The Phase 1 plus Q1 checkpoint uses merge `267946c29a` (with report-only descendant `d78e6ef0f0`). The same release Musenalm commands, ASCII build settings, and seven `ascii-off` queries as the starting baseline ran without concurrent compilation. The [raw checkpoint evidence](../../bench/results/musenalm/library-improvement-checkpoint-267946c29a/) preserves the schema manifest, build JSON, six query samples, plans, and run manifest. Corpus, schema, and workload fingerprints match the baseline, as do the fresh-build semantic index signature and all seven query answers, candidate counts, page sizes, source opens and record parses.

| Case | Baseline median ms | Checkpoint median ms | Delta ms | Source parses, both |
| --- | ---: | ---: | ---: | ---: |
| Q035 | 0.037 | 0.033 | −0.004 | 0 |
| Q045 | 0.176 | 0.191 | +0.015 | 0 |
| Q084 | 0.209 | 0.201 | −0.008 | 0 |
| Q127 | 0.061 | 0.059 | −0.001 | 0 |
| Q189 | 6.326 | 5.385 | −0.941 | 203 |
| Q190 | 114.186 | 77.607 | −36.579 | 3,821 |
| Q194 | 0.361 | 0.373 | +0.012 | 203 |

The fresh ASCII build measured 1,233.968→1,191.029 ms (−42.940 ms, −3.5%) and peak RSS 490.215→516.641 MiB (+26.426 MiB). The build interval has one observation per revision; the query interval has six samples per case. The checkpoint contains several integrated changes, so it cannot isolate their costs. Q190's large measured difference is much greater than its planning-time change and cannot be credited to Q1 without further controlled evidence. Small changes such as Q045 and Q194 are inconclusive at these sample counts.

## Estimate-only EXPLAIN contract

Q2 changes `explain()` to report physical path and candidate estimates without executing a candidate bitmap, relation witness, or inferred sort column. The trace marks candidate and sort figures as estimated and does not populate actual term output. `explain_analyze()` retains executable diagnostics and actual work counters. A counted-reader regression renames both source and target files after building the repository: relation and related-FTS `explain()` still succeed with no parser calls, while `explain_analyze()` reads the target. After the no-ff merge, that focused test, workspace tests (275 core unit tests passed, two ignored), and Clippy passed. This is structural correctness evidence; no Q2 timing was measured.

## Bounded inference summaries

B1 bounds short-string distinct summaries for declared and automatic-index-excluded paths using the existing distinct count and byte limits. When a summary exceeds either bound, its report states that the count is incomplete; the field's declared/opt-out status and inferred posting coverage remain unchanged. Fresh-build startup statistics now distinguish observed records, merge/finalization time, cumulative policy charges, repeated batch path charges, and retained manual-summary estimates. These estimates are not allocator measurements or a process RSS limit. Worker early-stop on a collection-wide failed inference result remains deferred because a safe deterministic cancellation boundary needs separate scheduler work.

The [matched synthetic build evidence](../../bench/results/library-improvements/inference-retention/README.md) compares `267946c29a` with B1 code commit `183d96ffad` on 1,024 JSONL files and 32,768 records, four build workers, ASCII, and snapshots disabled. One release sample per revision measured repository opening at 66.078→53.037 ms (−13.041 ms) and peak RSS 36,352→30,280 KiB. The fixture and inferred-index signatures, parsed record counts, and one admitted index match. This focused observation does not establish a repeatable speedup. After integration, the two focused bounds/worker tests, workspace tests (278 core unit tests passed, two ignored), and Clippy passed.

## Inferred-order page reuse

Q3 adds a generation-scoped lazy cache of immutable inferred `SortColumn` values. Its key includes collection, generation, field, locale, collation version, placement/collation options and direction. A 128 MiB estimate limits retained entries with LRU eviction; an in-flight `Arc` can outlive eviction, and the limit is not a process RSS ceiling. Fresh construction and snapshot restoration start with empty caches. The first construction holds the cache mutex, so concurrent first-use sorts could contend; the measured repeated-page case does not exercise that condition.

The [matched release samples](../../bench/results/library-improvements/inferred-ordering/README.md) compare Q2 code `0b0ea28f8f` with Q3 code `fc9beb2edf` on one 4,096-record UUID-like field fixture. Five fresh repositories each ran one first sorted page and ten repeat pages; all 55 requests returned the same 20 results with zero source opens and parses. First-use median was 4.245→3.850 ms (−0.395 ms, inconclusive over five samples). Repeat-page median was 4.173→0.091 ms (−4.082 ms, −97.8% over 50 samples), consistent with one cache build and later hits. Combined integration tests cover repeated non-ASCII strings, numeric/Boolean values, missing/null/fallback placement, both directions, count modes, eviction and restore. Three focused tests, workspace tests (281 core unit tests passed, two ignored), and Clippy passed.

## Prepared membership evaluation

Q6 prepares substring and escaped-percent patterns during binding and reuses them across scalar comparisons and inferred dictionaries. Membership-only FILTER and FTS routes stop after decisive matches and omit evidence sets when the request does not ask for evidence. Selected local and related FTS fields are evaluated through borrowed field views instead of cloning the bound FTS tree per field and record. The independent reference route uses the same requested evidence mode while retaining its separate record traversal. Focused tests compare wildcard semantics, three-valued fallback truth, local/relation/FTS membership and complete evidence; they assert one prepared pattern for 32 evaluations and one membership matcher call versus three evidence calls on three matching segments. After integration, two targeted tests, workspace tests (286 core unit tests passed, two ignored), and Clippy passed. Allocation bytes and Q6 query time were not measured, so the counter reduction is a structural finding only.
