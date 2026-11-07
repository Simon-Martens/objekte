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
| Q3 unlocked cold construction | `9ae442a633` | `f14fb9b8e2` | Two controlled concurrent-build tests, workspace tests and Clippy passed. |
| Q6 allocation evidence | `dba80c4df8` | `ce23f3f28e` | Evidence-only merge; matching fixture and nine result signatures checked, no code checks needed. |
| Q5 exact count and selection | `8830a19ed8` | `ca54a873f9` | Relation exact-selection test, workspace tests and Clippy passed. |
| Q4 local computed-sort scheduling | `773e26fe45` | `a1100d3828` | Worker/layout parity and large-record tests, workspace tests and Clippy passed. |
| Q4 related target load coordination | `1a074d746a` | `c06cf1eeda` | Related reuse/occurrence/dangling tests, workspace tests and Clippy passed. |
| R3 snapshot section assessment | `924abf3fd8` | `71b1b58397` | Diagnostic example compiled; workspace tests and Clippy passed; versioned format deferred. |
| O3 inference documentation | `5352c1d8aa` | `68469cb702` | Exported-default test, workspace tests and Clippy passed. |
| R1 execution context and typed failures | `9316761d31` | `5f0c32af6b` | Cancellation, resource, source-change and extension tests; combined workspace tests and Clippy passed. |
| R1 prepared-cache retention | `e2a3945286` | `da0edfb213` | Explicit retained-size tests and warm-regex budget tests; combined workspace tests and Clippy passed. |
| O1 execution organization | `00024fdf9e` | `92541ef900` | Compatibility re-export tests, workspace tests and Clippy passed. |
| O2 public library operations | `10fadc3679` | `fe4720ab4a` | Inferred posting, direct ID retrieval and exact count tests; workspace tests and Clippy passed. |
| R2 bounded JSONL framing and R4 freshness contract | `5347060b1a` | `645693821f` | Bounded framing/fallback/snapshot and stale-source tests; workspace tests and Clippy passed. |

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

Q3 adds a generation-scoped lazy cache of immutable inferred `SortColumn` values. Its key includes collection, generation, field, locale, collation version, placement/collation options and direction. A 128 MiB estimate limits retained entries with LRU eviction; an in-flight `Arc` can outlive eviction, and the limit is not a process RSS ceiling. Fresh construction and snapshot restoration start with empty caches. The follow-up constructs a missing column outside the cache mutex, then inserts it under a short lock. Concurrent same-key first uses can duplicate temporary construction, but publish one cached entry and reuse its `Arc`.

The [matched release samples](../../bench/results/library-improvements/inferred-ordering/README.md) compare Q2 code `0b0ea28f8f` with initial Q3 code `fc9beb2edf` and final unlocked code `81db11a03a` on one 4,096-record UUID-like field fixture. Five fresh repositories each ran one first sorted page and ten repeat pages; all 55 requests per revision returned the same 20 results with zero source opens and parses. Final first-use median was 4.245→4.052 ms (−0.193 ms, inconclusive over five samples). Final repeat-page median was 4.173→0.092 ms (−4.081 ms, −97.8% over 50 samples), consistent with cache reuse. Initial Q3 medians were 3.850 and 0.091 ms; the follow-up did not materially change repeated-page time. Combined integration tests cover repeated non-ASCII strings, numeric/Boolean values, missing/null/fallback placement, both directions, count modes, eviction, restore and two controlled concurrent first-use races. After the follow-up merge, its two focused tests, workspace tests (288 core unit tests passed, two ignored), and Clippy passed.

## Prepared membership evaluation

Q6 prepares substring and escaped-percent patterns during binding and reuses them across scalar comparisons and inferred dictionaries. Membership-only FILTER and FTS routes stop after decisive matches and omit evidence sets when the request does not ask for evidence. Selected local and related FTS fields are evaluated through borrowed field views instead of cloning the bound FTS tree per field and record. The independent reference route uses the same requested evidence mode while retaining its separate record traversal. Focused tests compare wildcard semantics, three-valued fallback truth, local/relation/FTS membership and complete evidence; they assert one prepared pattern for 32 evaluations and one membership matcher call versus three evidence calls on three matching segments. After integration, two targeted tests, workspace tests (286 core unit tests passed, two ignored), and Clippy passed.

The [matched allocator harness and raw samples](../../bench/results/library-improvements/prepared-evaluation/README.md) compare exact pre-Q6 code `b6d624ea57` and Q6 code `50760c9d0a`, with the same corpus fingerprint and nine result signatures. Across nine samples of 1,000 direct bound evaluations, scalar wildcard gross allocation requests fell from 5,000 calls/148,000 requested bytes to 2,000/36,000; array wildcard fell from 6,000/180,000 to 3,000/68,000. One warmed mixed relation/local FTS membership search per sample fell from a median 17,861 calls/1,347,731 requested bytes to 6,405/620,019. Scalar equality and mixed FILTER allocation-call controls were unchanged. The allocator counts whole operations and includes reallocation requests; requested bytes are neither retained heap nor RSS. One sequential process per revision does not establish a stable query-time speedup.

## Exact count and membership

Q5 adds an internal exact collector that counts index-proven candidates directly and accumulates residual exact-selection slots into a bitmap. Zero-limit count requests still validate their sort expression, then omit ranking, payload retention, evidence collection and computed-key evaluation because they return no page. The collection-scoped selection route resolves the query clock and shares the ordinary planner and scanner. Focused tests cover relation membership, sort validation, fallback and page/count parity; after integration, workspace tests (289 core unit tests passed, two ignored) and Clippy passed.

The [matched release evidence](../../bench/results/library-improvements/exact-membership/README.md) compares `f14fb9b8e2` with Q5 code `05daf7935e` on an identical 12,288-record fixture, five blocks per revision, one query/build worker, snapshots and verified cache disabled. Median exact count was 0.299→0.104 ms (−0.195 ms); count with computed `SORT BY body` was 8.507→0.087 ms (−8.421 ms); exact bitmap selection was 0.421→0.086 ms (−0.335 ms). All requests returned 12,288 exact matches. The sorted count's source opens, parses and sort-key evaluations changed from 1/12,288/12,288 to 0/0/0, explaining its large difference. This is a focused synthetic comparison, not the full Musenalm workload.

## Computed-sort scheduling, first commit

Q4's first commit restores bounded file parallelism for source-independent computed sorts, adds the computed field to the scan projection, and loads final page payloads after ranking. It reserves bounded per-file source/key work and applies results in file order for deterministic ties. Related computed sorts retain serial file groups until the separate target-cache work. Tests compare locator, bytes and logical pages with the reference route across one JSONL file and many files, workers 1/2/4/8, and an oversized record. The integration workspace suite passed (291 core unit tests, two ignored), as did Clippy.

The [matched release harness and raw samples](../../bench/results/library-improvements/computed-sorting/README.md) compare exact base `ca54a873f9` with code `9b27a2f50c` on 256 records, seven repeat samples per worker/layout, with identical fixture and ordered-answer hashes. For many local files, medians were 0.896→0.869 ms at one worker, 1.579→1.870 ms at two, 3.189→0.928 ms at four, and 1.978→0.600 ms at eight. The two-worker slowdown is explicit but its base/final ranges overlap (1.494–2.178 and 1.448–2.660 ms); it remains inconclusive. One-JSONL medians changed by 0.014–0.080 ms and do not show a file-concurrency gain. Related-sort results from this commit do not support a target-lock claim. The scan-retained payload estimate fell to zero, while some selected bytes are reread during final page materialization; neither counter is a process-memory measurement.

Q4's second commit allows related computed sorts to use those bounded file groups. A query-local per-target `OnceLock` coordinates duplicate target requests; the shared cache mutex covers lookup, budget reservation and counters while target reads and parsing run outside it. The 64 MiB target-cache estimate is reserved before I/O, with headroom retained inside the 128 MiB computed-sort estimate for parallel source work. Tests preserve FIRST/LAST occurrence order, dangling-target placement, page parity and high/low target reuse across workers 1/2/4/8. After the merge, both focused tests, workspace tests (293 core unit tests passed, two ignored), and Clippy passed.

The [paired commit-2 result](../../bench/results/library-improvements/computed-sorting/README.md#commit-2-result) compares `773e26fe45` with code `2b6fd5b7bd` using the same 256-record fixtures and seven repeat samples per case. All 16 related cases matched fixture hashes, ordered answers, counts and target-read/reuse counters. For many files with high target reuse, two/four/eight-worker medians were 1.589→0.890, 2.531→0.602 and 2.144→0.871 ms; with low reuse they were 2.618→1.198, 3.204→1.340 and 2.687→1.161 ms. One-JSONL results were flat within small variation because this route schedules by file. The memory figures are policy estimates, not measured peak RSS.

## Snapshot section scale decision

R3 adds explicit save-phase fields to the Musenalm build example; the snapshot writer and format are unchanged. The [single copied-corpus measurement](../../bench/results/library-improvements/snapshot-sections/README.md) built 148,486 ASCII records with snapshots disabled during opening, then explicitly saved 106.101 MiB in 488.892 ms. The largest collection section was 67.933 MiB, 13.3% of the existing 512 MiB section cap. Summed serialization/checksum/write/sync phases were 97.898/41.843/248.154/81.899 ms. A fresh-process restore took 469.271 ms with zero build parses. The measured corpus has ample section headroom, so a versioned chunked format is deferred. The current writer still buffers a section before its size check and cannot save a section above 512 MiB; no large-section acceptance claim is made. After integration, the diagnostic example compiled, workspace tests (293 core unit tests passed, two ignored), and Clippy passed.

## Inference documentation contract

O3 updates the inference guide to match the exported policy: 16 MiB per batch, 512 MiB coordinator and 768 MiB repository allowances, a 64-byte ordered short-string limit, and a 24 MiB per-path cap. It explains partial short-string coverage, bounded reporting for manual and opted-out paths, lazy inferred ordering, and the accepted JSON value model. A test asserts the documented defaults and 16-job allowance against `InferencePolicy::default()`. The no-ff merge passed the workspace suite and Clippy. No production behavior or benchmark result changed.

## Request execution context

R1 adds a per-request context with a frozen clock, cancellation token, typed failure classification and shared accounting for relation work, source reads, inferred ordering and computed sorting. Ordinary and streamed execution can use the same context; the existing API remains available. Tests exercise cancellation during relation and computed-sort work, stale-source and extension failure classes, and bounded source/extension values. The limits are estimates of selected retained state and reads, not a process RSS ceiling. A reader without a preparse estimate can allocate while parsing before its expanded value is rejected. No matched R1 timing was recorded.

The paired prepared-cache change replaces debug-string sizing with explicit retained-state estimates, including bound literals, gram requests, matchers and compiled regex. The regex charge uses configured compile/cache ceilings because the pinned regex API does not expose precise compiled allocation size. The default prepared-cache allowance rises from 4 to 16 MiB so a normal prepared regex query is reusable under that conservative charge; it allocates no cache memory up front. The combined integration tree passed `cargo test --workspace` (311 core tests passed, two ignored) and `cargo clippy --workspace --all-targets` with existing warning-level diagnostics.

## Execution module organization

O1 extracts payload conversion into `materialization.rs` and work-counter accumulation into `measurement.rs`. Runtime result, sort and alphabet options now live in `execution_options.rs`; `workload.rs` retains compatibility re-exports for existing callers. The change preserves query and serialization behavior and makes no speed claim. After integration, the workspace suite passed (312 core tests, two ignored) and Clippy exited successfully with warning-level diagnostics.

## Public library operations

O2 exposes eligible inferred posting dictionaries through the existing bounded value-page API. Listed-key counts remain exact; coverage now reports indexed and unresolved records and marks partial dictionaries as incomplete facet enumerations. A new collection-and-ID lookup returns locator, bytes or logical payload with the usual stale-source and raw-fallback behavior. Exact count also has ordinary and execution-context entry points, using Q5's zero-page count route. Focused tests include mixed short and long array values: a listed short key has an exact count while the overall dictionary remains incomplete. The integrated workspace suite passed (315 core tests, two ignored) and Clippy exited successfully.

An opaque reusable prepared-request handle remains deferred. The current internal prepared tree binds after parameters and clock resolution while the executor accepts `Query`; a public handle needs a clear template, frozen-clock and repository-generation contract before it can safely reuse that state. No O2 query-speed claim is made.

## Bounded large-file construction and freshness

R2 frames oversized built-in JSONL sources into ordered build chunks, ordinarily at most 1 MiB and 256 records. A single longer line forms its own chunk. Custom readers retain whole-buffer framing unless they explicitly declare the same newline contract. Stable-input tests cover absolute ranges, blank lines, malformed siblings, diagnostic coordinates, postings, bigrams and snapshot restoration. R4 documentation now describes fresh-build stability assumptions, explicit snapshot saving, and the actual query-time revision probes, including buffered reads and replacement limitations.

The [matched large-file evidence](../../bench/results/library-bounded-framing-20261107/README.md) compares exact pre-R2 `da0edfb213` with final R2 code `4043acc23d` on one 100.901 MB JSONL file, 8,192 records, ASCII, four build workers and snapshots disabled. Three alternating fresh processes per revision had identical semantic signatures and 8,192 parses. Median opening wall time was 493.299→430.717 ms (−62.582 ms, −12.7%); median peak RSS was 102.145→24.082 MiB (−78.062 MiB). The admitted-weight estimate fell 104.213→11.952 MiB. Post-open resident memory increased 9.078→23.016 MiB, so the result concerns peak construction memory rather than retained memory. Combined workspace tests passed (323 core tests, two ignored) and Clippy exited successfully. Query record-batch scheduling was unchanged.
