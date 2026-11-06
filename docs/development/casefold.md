# Objekte core: shared case-folded bigram index implementation plan

## 1. Assignment

Change bigram filtering so that each indexed text representation uses one case-folded bigram index for both case-sensitive and case-insensitive searches. Fold the text supplied to the bigram constructor and the corresponding query text using the same Unicode case-folding rules. Exact verification retains the query's case semantics.

The purpose is efficient case-sensitivity switching without constructing or retaining a second case-sensitive index. Alphabet selection remains an independent repository setting: ASCII or Unicode determines which grams can be represented, not the exact query's case behavior.

This plan authorizes the minimum query-request and binding changes necessary to select case sensitivity, including unscoped FTS. Preserve existing query defaults and FILTER grammar. Do not implement relations, regex, other query-language features, runtime updates, new production formats, UI controls, or a new extension framework. Do not add a selectable case-preserving bigram index or enumerate case variants. Exact postings, logical values, returned bytes, source text, sort keys and file metadata are outside the folding change.

The inspected branch is `development/local-file-search-core`, head `29720036d896e6c6500df444b61a0807a543d60c`. The existing complete benchmark measures code `92cdb92b19200cb0005af23b66aeb3cd1b514c4b`. Inspect the actual checkout and applicable `AGENTS.md` before implementation. Preserve any subsequent foundation/performance repairs; do not roll back to the inspected revision or mix their unrelated fixes into these commits.

Create a separate branch from the current development state, suggested name `perf/core-shared-casefold-bigrams`. Commit and push every completed step separately, with focused tests accompanying the relevant fix. Preserve unrelated work; do not force-push or merge automatically. Record push failures and retain local commits if the remote is unavailable.

## 2. Current behavior and code locations

| Responsibility | Starting location / symbols | Current issue or obligation |
| --- | --- | --- |
| Bigram construction and lookup | `crates/core/src/bigram.rs`: `BigramIndex::add_record`, `query_grams`, `candidates`, `estimate_min_cardinality`, `ascii_pair`, `unicode_pair` | ASCII keys lowercase printable ASCII bytes; Unicode keys preserve the supplied scalar values. Both construction and lookup must change consistently. |
| Index construction and custom text | `crates/core/src/repository.rs`: raw, fallback and `custom_text` construction | Fold only the index input after the registered extraction/transformation; retain reader-owned text and original values. |
| FILTER access paths | `crates/core/src/executor.rs`: `enumerate_paths`, `custom_bigram_compatible`, `custom_candidates`, raw-coverage helpers and path execution | `lower` is currently excluded from useful bigram paths. Replace name-based exclusions with valid representation/coverage checks. |
| FTS planning | `executor.rs`: `fts_candidates`, `estimate_fts_candidates` | Selected-field `lower` currently returns no bigram restriction. Estimates and execution must use the same new rules. |
| Query semantics and exact matching | `crates/core/src/query.rs`, `reference.rs`: `FtsQuery`, bound terms/FTS, `bind_filter`, `bind_fts`, `compare_scalar`, `text_contains` | Preserve original sensitive literals; keep exact matching separate from folded candidate preparation. |
| Verification and fallback text | `crates/core/src/run.rs`, `repository.rs` reference execution, `record_preparation.rs` | Apply the chosen exact mode on every route, including raw fallback; preserve single-decode behavior. |
| Query/result caching | `crates/core/src/prepared.rs`, `verified.rs` or current verified-cache module | Sensitive and insensitive results must remain distinct even when their gram requests are identical. |
| Persistence | `crates/core/src/storage.rs`: `configuration_hash`, snapshot compatibility; `BIGRAM_VERSION` | Reject/rebuild old bigram representations and validate the new metadata. |
| Workload and examples | `workload.rs`, `crates/core/examples/bench_musenalm.rs`, `examples/support/musenalm.rs` within the core crate | Retain old defaults and workload; add focused case-mode pairs outside the fixed 150-query list. |

Use these as a responsibility map, not a requirement to retain every file name. Keep structural changes small and deliberate.

## 3. Required semantics

### 3.1 One index representation; two exact modes

Let `T` be the registered text representation of one segment and `F` the selected Unicode full case-fold operation. Store ordinary and skip bigrams from `F(T)`. Derive query grams from `F(Q)`, where `Q` is the query literal prepared for that same representation.

| Stage | Case-sensitive exact matching | Case-insensitive exact matching |
| --- | --- | --- |
| Candidate index | Grams from `F(T)` | The same grams/index |
| Candidate request | Grams from `F(Q)` | The same gram request for equivalent folded literals |
| Exact substring verification | `T.contains(Q)` | `F(T).contains(F(Q))` |
| Equality verification | `T == Q` | `F(T) == F(Q)` |

These formulas describe the built-in string operations. Preserve the existing wildcard, array-quantifier, missing/null, unavailable-value, and custom-matcher semantics around them.

For fixed default full case folding, a literal substring in the original text remains a substring after applying the same fold to both strings. Therefore folded grams are necessary conditions for sensitive matches as well as insensitive matches. They are not sufficient conditions. A sensitive query for `Der` may select a record containing only `der`; the scanner must reject it.

Do not label a bigram result exact, infer exact field evidence from a gram hit, or complement approximate candidates for NOT/inequality. Index-proven answers from compatible typed postings remain exact under their existing contracts.

### 3.2 Use full Unicode case folding

Reuse the existing `unicode_casefold` dependency and the current scanner's folding policy, rather than adding another normalization library. Make the chosen full/non-Turkic policy explicit and shared. Confirm the pinned dependency and its tables locally. Record a representation version that changes when relevant folding behavior changes; do not silently upgrade Unicode tables during this task.

Full folding can expand a character, for example `ß` to `ss`. It is not equivalent to lowercasing. Use folding for internal search preparation only; never overwrite logical values or display folded text as the original payload.

Do not add Unicode normalization, accent removal, punctuation removal, whitespace changes, or locale-sensitive casing. Existing registered transformations retain their contracts. Case folding alone does not promise canonical-equivalence matching for composed versus decomposed strings.

### 3.3 Alphabet and gram boundaries

Apply folding before gram generation, including skip grams. Never fold already constructed pair keys: expansion changes both adjacent and skip relationships.

Once callers supply the folded stream, keep pair-key functions as encoders. Remove redundant hidden ASCII lowercasing where doing so clarifies ownership, while retaining the same keys for already folded ASCII input.

Unicode mode emits pairs of folded Unicode scalar values. ASCII mode retains its existing printable-ASCII eligibility and byte-window rules over the UTF-8 encoding of the folded stream. Do not combine this task with a redefinition of ASCII skip distances. Implementations using a streaming encoder must reproduce those byte-window rules exactly.

Unsupported bytes/characters do not become adjacent by deletion. Preserve positions when deciding which ASCII normal/skip windows are usable. If a literal produces no usable necessary grams, use the existing unrestricted/scanner fallback; do not return an empty match set.

Reset gram history at every reader-owned text segment boundary. Do not construct grams across field values, array elements, separate records, or unrelated extracted segments. Preserve the existing union-of-segment record membership semantics.

Determine usable gram length after folding. A one-character source literal such as `ß` can yield two folded characters and usable grams; `a` and `ü` still yield one character and no ordinary pair. Empty literals retain their established query semantics.

## 4. Query preparation and exact case selection

### 4.1 Keep exact and candidate literals separate

Do not replace a sensitive bound term's literal with its folded spelling. Keep the original/semantically prepared exact literal and a separate prepared candidate representation, or an equivalent typed distinction that prevents accidental use of folded text for sensitive verification.

A suggested internal split is:

```text
Prepared string predicate
  exact literal / exact matcher
  exact case mode
  field, quantifier and registered representation
  prepared folded gram request
  coverage and compatibility requirements
```

Prepare the folded query and its gram keys once per relevant prepared representation, not per candidate record or repeatedly in both estimator and executor. Binding-specific parsing/transformation happens before folding for candidate generation. Reuse existing prepared-query caching and update memory accounting for any newly retained strings or key arrays.

Idempotence permits reuse of already-folded built-in input, but do not casually apply custom transformations twice. Retain a clear owner for binding transformation and a clear owner for bigram folding.

### 4.2 FILTER

Keep existing sensitive predicates sensitive. Keep `:lower` as the existing case-insensitive operation; it currently means Unicode case folding despite its spelling. Named `:folded` registrations continue to work and need no user migration.

Enable compatible shared folded bigram paths for positive string equality/contains predicates under the same operator and quantifier restrictions that make the path sound today. Do not broaden wildcard or universal-array optimizations without an independent proof. Existing safe scanner fallback for `%` and escape-containing patterns may remain; the case-mode change does not require a wildcard planner.

Never fold ordinary posting dictionary keys globally. A case-sensitive posting equality cannot prove insensitive equality merely because the literal was folded. Preserve existing exact posting/dictionary paths and custom-posting compatibility.

### 4.3 FTS, including unscoped searches

The reviewed `FtsQuery` contains a literal and selected fields but no independent case-sensitivity field. If the current checkout still lacks one, add a small internal `CaseSensitivity` enum with `Sensitive` and `Insensitive`, and an FTS request field defaulting to `Sensitive`. Reuse an existing equivalent if later work already supplies it. Do not create another query parser or an inline FTS syntax.

Thread this choice through ordinary unscoped FTS, selected-field FTS, binding preparation, optimized execution, the independent reference route, cache keys, and examples. If workload serialization needs a field, add an optional/defaulted `fts_case_sensitivity`; all existing 150 query entries must retain their present interpretation without edits. The FTS case mode must not silently change attached FILTER predicates.

Case sensitivity applies to the selected registered text representation. A binding that already folds or transforms the text retains that behavior; selecting sensitive mode does not undo the binding. Document this ordering. If a custom exact matcher does not support the newly requested mode, reject that unsupported combination during preparation rather than silently changing its semantics or pretending its old result is correct.

Apply the same exact mode to unscoped raw-fallback text after its established recovery decoding. Keep selected logical-field FTS unavailable on raw fallback records, and preserve metadata/Boolean availability semantics. Known malformed records must not be reparsed to verify FTS.

The mode is selected per request against an already built repository. Switching modes must not rebuild an index, modify registrations, or accumulate a second bigram store.

## 5. Index construction and allocation discipline

Implement the shared folding/gram preparation in the bigram component or one small internal helper used consistently by construction, query preparation, estimation, and lookup. Avoid separate subtly different ASCII, Unicode, raw, and custom folding implementations.

Use the existing extraction pipeline. Fold each segment for index construction after its binding transformation. Keep original extracted segments available to exact matching and return original logical/byte payloads. The default healthy `raw` target remains extracted logical text; malformed fallback remains its separate recovery representation.

Use an iterator with a short rolling history where practical. An ASCII-only fast path is allowed if it produces exactly the full-fold result for that input. For the existing ASCII byte-window contract, either stream the folded scalars' UTF-8 bytes through the same windows or use a reusable scratch buffer. Do not allocate both a full folded string and a full `Vec<char>` for every segment when one bounded/reused buffer or iterator suffices.

Retain per-record gram deduplication and normal/skip stores. Bound scratch storage to current processing needs; do not retain folded full-record text for the repository lifetime. Avoid a virtual callback per character or a mutex per gram.

Case-insensitive exact verification may need folded text, as it already does. Reuse folded segment preparation within one evaluation where several predicates need the same representation, if this can be done within existing bounded preparation. Sensitive verification must not incur a new full-text folding allocation solely because the index is folded. Do not introduce a repository-wide folded-text cache as part of this change.

## 6. Planner compatibility and index reuse

### 6.1 Replace the current blanket exclusions

Update raw and custom access-path enumeration, actual path execution, cardinality estimates, FILTER, and selected-field FTS together. Remove unconditional exclusion of `lower` only where the chosen index actually covers the required text representation.

A compatible shared folded index is available to both sensitive and insensitive exact verification. Estimate using the folded gram request and actual folded posting cardinalities; sensitive queries can now have more candidates than before. Keep cost-based term ordering and the option to choose a cheaper exact posting path or scanner.

### 6.2 Preserve extraction and coverage checks

Folding compatibility does not prove extraction compatibility. Preserve checks for reader identity/version, logical-text coverage, field/segment coverage, selected custom transformation, generation/domain, and matcher compatibility.

In particular:

- A body-only raw index cannot filter an omitted title, even if both are folded.
- A raw binding that removes spaces is not automatically usable for a default field predicate whose representation retains spaces.
- A custom matcher with semantics beyond literal/equality matching needs its existing explicit conservative-compatibility guarantee.
- Partial coverage must keep uncovered eligible records in the candidate domain. Malformed records cannot leak into logical proofs or complements.
- Metadata predicates should retain their existing inventory/index paths without reading source text.

Do not remove the foundation's compatibility guards to make `:lower` faster.

### 6.3 Share equivalent representations without duplicating indexes

The requirement is one folded bigram store per distinct indexed text coverage/representation, not one store for the entire repository. Raw collection text, a field-only text target, and a genuinely different custom representation can still need separate stores.

For the same extracted segment stream and coverage, identity text followed by mandatory index folding and a built-in case-fold binding followed by index folding have the same candidate representation. Canonicalize or alias those known-equivalent candidate registrations so toggling case or using the compatible built-in `lower`/`folded` spelling does not create another physical normal/skip pair of stores.

Do not infer arbitrary custom equivalence from a binding name, a few samples, or equal counts. Keep custom representations separate unless their contract establishes compatibility. A field-only folded index and whole-record raw index differ in coverage and selectivity; they are not duplicates merely because both use case folding.

Prepared/explain output should identify alphabet, folded representation/version, covered target, exact case mode, selected access path, estimated candidates, and remaining verification. Keep this diagnostic preparation outside primary timed samples.

## 7. Cache and snapshot correctness

### 7.1 Cache identity

Audit prepared and verified cache keys. They must preserve the exact predicate semantics, including case mode, original sensitive literal, collection/field, binding/version, operators, Boolean structure, and existing generation scope. A prepared object with a folded gram request is not a case-insensitive exact query by definition.

Do not key verified membership only by folded literal or candidate bitmap. Sensitive `Der`, sensitive `der`, and insensitive `der` may share gram requests but have different answers. Test alternating those queries on the same repository with caches enabled, including counts, sorted pages, evidence and logical/byte payloads.

Reusing immutable candidate preparation is optional; do not add another cache solely for this task. Keep the existing separation between verified membership and payload loading.

### 7.2 Persistence

Bump `BIGRAM_VERSION` and ensure it participates in configuration/snapshot compatibility. Bump the physical snapshot version if encoded layout changes; a semantic version/hash incompatibility is sufficient where the existing mechanism reliably rejects the old representation. Do not unconditionally change unrelated format versions.

Persist or fingerprint the alphabet and folded-representation policy/version, along with existing extraction/binding/domain identity. If an explicit field is added, use a validated enum/version rather than a mutable unvalidated flag.

An old case-preserving Unicode snapshot must be rejected/rebuilt through the normal compatibility route. Do not relabel old keys, lowercase keys in place, or merge existing pair columns as a migration: full folding can expand characters and change adjacency and skip grams. Rebuild from source using the established pipeline.

Version any changed persistent prepared/result-cache contract if such caches exist in the actual checkout. Fresh build and restore must use identical mode-aware query behavior. Switching modes on a compatible loaded generation must never trigger snapshot rebuilding.

## 8. Focused correctness and execution tests

Add tests beside each change. Use small independent expected-answer fixtures, then exercise optimized and forced-scan routes. Do not rely only on two paths that share the same comparator.

| Case | Required observation |
| --- | --- |
| `Der`, `der`, `DER`, mixed-case distractors | Both modes use folded candidates; sensitive verification rejects wrong case; insensitive verification accepts the intended variants. |
| `Müller`, `MÜLLER`, `müller` | Same built repository serves both modes, with actual Unicode bigram use. |
| `Straße`, `STRASSE`, `straße`; literals `ß` and `ss` | Full-fold expansion is indexed before normal/skip gram generation; exact sensitive answers remain distinct. |
| Greek `Σ`, `σ`, `ς` | Expected full-fold behavior; no false negatives from inconsistent folding implementations. |
| `I`, `i`, `İ`, `ı` | Explicit non-Turkic expectations, including expansion/combining-dot behavior; no accidental locale dependence. |
| Composed/decomposed accents | No unrequested normalization or accent removal; preserve the existing exact semantics. |
| Empty, single-scalar, no-case-script and emoji literals | Correct unrestricted fallback where no usable grams exist; no false empty result. |
| `aßb` and surrounding literals | Ordinary and skip grams follow the expanded folded stream, including boundaries around `ss`. |
| Separate segments `ab` and `cd` | No generated cross-segment `bc` constraint or fabricated exact cross-segment match. |
| Unsupported characters between ASCII characters | Preserve ASCII window distances; never obtain restrictive grams by deleting unsupported input. |
| FILTER equality, contains, negation and arrays | Exact scalar/quantifier rules unchanged; no complemented approximate candidates. |
| Unscoped FTS and selected-field FTS | Case mode reaches preparation, planner and exact verification; attached FILTER retains its own semantics. |
| Custom raw/field bindings and excluded fields | Valid equivalent reuse works; incompatible or incomplete coverage does not prune real matches. |
| Healthy and malformed JSONL siblings | Correct recovery representation, status, evidence and unavailable logical values in both modes. |
| Warm cache with alternating case modes/literals | No cross-mode result contamination; correct counts, pages and payloads. |
| Fresh build, restored state and old snapshots | Same answers for compatible state; incompatible representation rejected/rebuilt. |

Add deterministic candidate-superset properties for generated short strings using an independent simple reference fold in tests if practical, or explicit expected folded strings plus a separate reference enumeration. For each supported sensitive/insensitive exact match, the record must remain in candidates or the query must report unrestricted fallback. Include expansions, unsupported ASCII windows, segment boundaries and short queries.

Add structural assertions: `:lower` no longer automatically selects a full scan when a compatible useful folded index exists; selected-field and unscoped insensitive FTS actually consult it; repeated case toggling leaves index-store count and construction counters unchanged; folded aliases do not build duplicate stores over identical coverage. Do not assert that an index must be chosen when its estimated work is worse or it has no usable grams.

Preserve exact postings' zero-source-read tests, shared preparation, bounded source access, one-decode output reuse, and corruption tests from earlier work. Use counters rather than millisecond thresholds in unit tests.

## 9. Benchmark policy and acceptance

### 9.1 Before-state

Reuse a compatible recorded current-state benchmark. At the inspected revision, the available complete runs are `bench/results/musenalm/final-92cdb92b19/` and `final-repeat-92cdb92b19/`, with build/restore and controlled/focused recordings alongside them. If the performance-repair assignment has since changed the code, use its final recording instead.

Record the exact starting SHA, corpus/schema, resolved queries, compiler/build flags, machine, worker count, cache settings and timing boundaries. Run a new full baseline only if no applicable current-state recording exists; explain the incompatibility. New focused insensitive-mode cases without old equivalents are first measurements, not invented historical speedups. Where possible compare them against the old equivalent binding/scanner semantics.

### 9.2 Focused measurements during implementation

Use the same loaded repository for alternating case-sensitive and insensitive requests, with verified-result caching disabled for the primary candidate-filter comparison. Also test warm-cache mode correctness/performance separately. Use paired/alternating old/new process blocks for timing attribution; keep compiler flags, corpus, CPU policy/affinity, worker count and filesystem-cache conditions identical.

Measure both alphabets. Include ordinary raw FTS for `Der`, `Liebe`, `Goethe`, `Müller`, `Frühling`, and a mixed-case synthetic fixture, plus selected-field FTS and FILTER pairs for `name ~ 'Goethe'`, `name:lower ~ 'goethe'`, `name:folded ~ 'GOETHE'`, and `preferred_title:lower ~ 'FRÜHLING'`. Include single-character and expansion cases, selective and broad results, exact-posting controls, and combined FTS/FILTER queries.

Record absolute median milliseconds and variability, candidates, actual source opens, scanned records, parses, exact matches, total matches and returned page size. Distinguish construction work from per-query work. Use coarse profiling or separate diagnostic runs to attribute folding allocation costs; do not add per-character timers to production paths.

Expected tradeoff: insensitive queries can become substantially cheaper by gaining a useful index, while sensitive Unicode queries can become slower because folded candidates merge cases. The current candidate counts for `Der`/`Müller` are historical observations, not promised post-change counts. Quantify both directions. Do not claim every query should become faster or that larger candidate counts are automatically a defect in this design.

If sensitive performance degrades materially, report its absolute cost and whether it comes from expected candidate growth or avoidable implementation overhead. Fix avoidable overhead. Do not silently add a second sensitive index, change semantics, or hide the tradeoff behind a warm-cache aggregate.

### 9.3 Mandatory final measurements

Run focused checks per commit; do not add a new 150-query regression suite or repeat the full workload after each step. Once code and correctness checks are final, run the existing complete 150-query benchmark through its established protocol: ASCII/Unicode, verified cache off/64 MiB warm, five timed repetitions, and the applicable baseline's source/decoded-cache and worker settings. Keep all existing queries unchanged.

The fixed workload does not cover every new insensitive FTS mode, so save a separate small paired-mode benchmark alongside it. Include enough cases to demonstrate that ordinary unscoped FTS can toggle case sensitivity without index reconstruction. Do not replace the full historical workload with this subset.

Measure fresh index construction and fresh-process restore before/after for both alphabets using applicable existing recordings and the established repetition counts. Record phase timings, snapshot bytes, resident/peak memory, distinct gram/store counts, parser counts, and folding/allocation observations. Reuse of equivalent representations should not produce a hidden second full index.

Save raw samples, query JSON, plans/counters, correctness results, environment and exact measured SHA. Print and save the complete final per-query table in milliseconds, with full query expressions/options, source-access/scan counts, total matches and returned page sizes. Report absolute differences first, percentages second. Keep profile and case-mode labels explicit.

Run the normal formatting, lint and full relevant tests. Use the existing independent acceptance/oracle where applicable and explicit Unicode/mode fixtures for semantics the oracle does not model. The final report must distinguish unchanged query answers, intended new mode behavior, selectivity losses, overhead reductions and unresolved regressions.

## 10. Phases and commits

| Step | Suggested commit subject | Completion evidence |
| --- | --- | --- |
| 1 | `docs(search): define shared case-folded bigram semantics and baseline` | Starting revision, preserved defaults, folding/coverage contract, baseline applicability and focused cases. |
| 2 | `feat(query): carry explicit FTS case sensitivity through exact evaluation` | Existing equivalent reused or minimal typed option added; ordinary/selected/raw-fallback/reference paths and cache identity tested. Scanner fallback is acceptable until step 4. |
| 3 | `perf(bigram): construct shared folded grams for both alphabets` | Construction and lookup changed together; expansion/window/segment tests; version compatibility updated in the same coherent commit. |
| 4 | `perf(planner): reuse folded bigrams across case modes` | FILTER/FTS estimates and execution aligned; coverage safeguards retained; compatible lower/folded paths and aliases verified. |
| 5 | `test(search): verify case-mode parity across cache and restore paths` | Cross-feature matrix, candidate-superset tests, old-snapshot rejection and resource invariants. |
| 6, if required | One precise performance-fix commit per measured overhead | Focused before/after evidence for allocations or execution work; no unrelated refactoring. |
| Final | `docs(bench): record shared case-folded index results` | Final existing 150-query run, paired modes, build/restore, memory, printed/saved full tables and explicit tradeoffs. |

Tests accompany their implementation commits; step 5 adds interaction coverage. If a new FTS mode needs more than one commit, each published intermediate state must either execute it correctly or reject it explicitly. Never expose a mode that silently ignores the requested semantics.

In particular, step 2 must not send a folded insensitive literal to an existing case-preserving raw index. Until the new representation and planner support are installed, use a conservative scanner path for that mode unless an already compatible folded index is explicitly available.

Each commit is separately pushed when possible. The final documentation commit identifies its measured code parent. Subsequent code changes require relevant validation and measurements at the new revision; documentation-only edits do not require retiming unchanged code.

## 11. Definition of done

- One folded normal/skip bigram store per distinct indexed text coverage/representation serves both exact case modes.
- ASCII and Unicode use the same explicit folding policy before their own gram encoding, with correct expansion and segment handling.
- Sensitive queries preserve original exact comparisons; insensitive queries use the agreed full-fold semantics.
- Ordinary FTS can choose case sensitivity per request, and compatible `:lower` and selected-field paths obtain index assistance.
- Toggling mode neither rebuilds indexes nor creates a duplicate sensitive/folded store for identical coverage.
- Custom representation compatibility, exact postings, unavailable-value semantics and source-access bounds remain correct.
- Cache identity distinguishes exact modes and sensitive literals; old index snapshots cannot be misinterpreted as the new representation.
- Correctness and structural execution assertions pass; final query/build/restore evidence is saved, printed, and tied to the measured revision.
- Any material case-sensitive slowdown is quantified explicitly, with expected selectivity cost distinguished from implementation overhead.

## 12. References

- Reviewed implementation: [bigram encoding](https://github.com/Simon-Martens/objekte/blob/92cdb92b19200cb0005af23b66aeb3cd1b514c4b/crates/core/src/bigram.rs), [planner](https://github.com/Simon-Martens/objekte/blob/92cdb92b19200cb0005af23b66aeb3cd1b514c4b/crates/core/src/executor.rs), [query binding and exact matching](https://github.com/Simon-Martens/objekte/blob/92cdb92b19200cb0005af23b66aeb3cd1b514c4b/crates/core/src/reference.rs).
- Existing evidence: [foundation report](https://github.com/Simon-Martens/objekte/blob/29720036d896e6c6500df444b61a0807a543d60c/docs/development/foundation-results.md).
- Unicode semantics: [Unicode case-mapping/case-folding FAQ](https://www.unicode.org/faq/casemap_charprop.html).
- Existing dependency API: [unicode-casefold 0.2.0, UnicodeCaseFold](https://docs.rs/unicode-casefold/0.2.0/unicode_casefold/trait.UnicodeCaseFold.html). Confirm the repository's pinned dependency before implementation.
