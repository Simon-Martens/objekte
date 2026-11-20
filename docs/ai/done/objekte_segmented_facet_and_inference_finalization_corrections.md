# Objekte repair review and remaining corrections

**Decision.** Keep the empty-string and length query repairs and the provisional-observation ownership change. Hold integration for the remaining public facet and inference-report errors. Construction still does avoidable work at segment sealing. This addendum targets the code on `repair/path-posting-query-evaluation`, implementation `3ece7c5daf`, report tip `0345baa3e94d69efbabd6a4235eaf1e3a312481e`. Make the corrections in the existing repair worktree. The owner integrates the result.

## Evidence already accepted

- The empty-string regression compares optimized and reference record IDs for default, existential and restricted queries. It retains the successful empty traversal for the default comparison. A simple scalar control asserts zero source opens and parses. The length regressions cover multiple terminals, top-level empty arrays, nested empty arrays, missing paths, a separate segment, equality and inequality, with file-free simple controls. Leave these fixtures in place. [Query tests](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/inference/tests.rs#L139)
- The existing numeric facet case, with one segment missing `x`, is repaired. The current order test constructs its collection metadata once and reverses the physical segment list passed to the reader. It cannot detect a field-family choice that changed while building that metadata. The new mixed fixture below must construct the collection summary independently in each order. [Order test](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/repository/posting_values/tests.rs#L704)
- Interrupted single-source projection correctly preserves `IncompleteObservation` across a second projection. Completed publication sheds its construction observation. The lifecycle test checks zero staged bytes, shared native payload identity, and weak-reference expiration after the provisional reader is dropped. [Projection regression](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/index/segment/compaction.test.rs#L483) · [Ownership regression](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/lifecycle/initial/tests.rs#L257)

## 1. Resolve facet identity before consulting inference

At `repository/posting_values.rs:888`, a segmented request with an empty base enters the finalized inference resolver before the ordinary resolver. That shortcut indexes the report by `path.segments` and inspects only the binding explicitly supplied in the request. Two parts of the facet's identity can be lost.

| Regression | Fixture and expected public result |
| --- | --- |
| Reserved path root | Build an AutoIndex collection admitting numeric `x`. A request for `x` enumerates 7. A request for `@backlinks.x` must return the existing logical-data-field error. Both paths have the same field-name vector. Their roots differ. If using a scalar-root collection, also assert that a metadata request cannot alias the admitted `@root` dictionary through the empty field-name vector. |
| Schema-selected named binding | Register string field `title` with `indexed: false`. Register the existing complete test posting operation `fold`, make it the schema default, and explicitly prohibit automatic indexing of the raw field. Ingest `HELLO` and `hello`. An omitted binding and an explicit `fold` request must each return one lowercase `hello` key with count two and identical coverage. An explicit `default` request for the prohibited raw index must still be rejected. |

Validate `PathRoot::Data` immediately after parsing in the shared facet request path or before **every** segmented resolution branch. Compute the effective binding once using explicit request → schema default → literal `default`. Pass that identity to the ordinary, finalized and provisional routes. Consult inferred admission only for the effective raw/default request on an eligible field. A rejection of the automatic raw index must not override a registered custom operation. Preserve the binding version in cursor validation. Reuse the existing named-binding test operation and the baseline's error behavior. The current named-binding test sets `indexed: true`, which bypasses the faulty branch.

[Segmented dispatch](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/repository/posting_values.rs#L888) · [Finalized policy](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/repository/posting_values.rs#L166) · [Ordinary resolver](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/repository/posting_values.rs#L266) · [Field path parser](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/model/mod.rs#L545)

## 2. Make the facet family independent of segment order

Create two one-record segments, one with `{"x":7}` and the other with `{"x":"a"}`, and admit the short-string dictionary with AutoIndex. Assert the global observations include both scalar kinds, the general dictionary is incomplete, and short-string candidates are available. Perform **two builds** (or use the existing real collection aggregation on both physical orders) before calling the public facet API. Verify the active segments actually have the intended local families. Compare with the ordinary one-segment partial-string result.

**Failure mechanism.** With multiple observed kinds, the new helper falls back to `diagnostic.kind`. The producer assigns that from `aggregate.indexes.first()`. Numeric-first can therefore ask for a numeric dictionary with short-string coverage. Neither local dictionary satisfies both requirements. String-first can choose a string representative. In the residual scan, the temporary posting accepts scalar keys by their actual value type, and the final short-string filter removes only over-length strings. A numeric key can consequently escape into a string facet. Enumeration intersects the key's posting with the active request domain, independently of the coverage bitmap.

1. Derive the policy from the admitted collection capability. A short-string-only facet has string keys and the configured UTF-8 byte limit, even when other scalar kinds were observed. A full homogeneous value facet uses its established value family. Keep both policies distinct when a general dictionary and a short-string acceleration coexist. Use the finalized policy as the authority during candidate selection, source fallback, bound checks and enumeration. Use local posting kinds only to check compatibility.
2. In short-string fallback, keep only eligible string keys as ordinary enumerable entries and preserve any internal null evidence used for coverage. Non-string and over-limit values must not appear as enumerated keys. Their records may still contribute presence and unresolved coverage. For the two-record fixture both orders must return just string `"a"` with count 1, two eligible, two present, one indexed, one unresolved, `value_counts_complete == false`, and no continuation. Verify a selection restricted to the numeric record yields an empty value page with one unresolved record. Preserve complete dictionaries' existing long-string behavior.
3. The local fallback at `posting_values.rs:923` uses `find_map` over `Option<Result<…>>`. A physically present path that fails resolution produces `Some(Err)` and ends the search. For an eligible provisional or update-only path with no applicable finalized decision, continue past unsuitable local dictionaries until a compatible one is found. Keep actual registration/version errors for the selected binding distinguishable. Add a local-fallback case whose first active segment has only a structural or length dictionary and whose later segment holds the admitted enumerable family. Preserve final opt-out and capability rejections.
4. Reuse the existing numeric-absent-segment, partial coverage and pagination tests. The acceptance oracle for the mixed case is identical values and coverage across independently aggregated layouts. A test that changes iteration order after building one global diagnostic cannot establish that property.

[First-index diagnostic](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/construction/inference.rs#L423) · [Policy and representative](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/repository/posting_values.rs#L210) · [Fallback key retention](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/repository/posting_values.rs#L520) · [Local fallback](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/repository/posting_values.rs#L923)

## 3. Propagate an interrupted contributor to occurrence-count status

The existing report-combination code iterates only the field rows present in each input. If one nonempty input stopped observing before seeing field `x`, it has no `x` row. Another input's exact status can survive even though the result covers the unobserved records. The combination sets the overall `report.complete` to false but leaves the field's individual `occurrence_count_status` at `Exact`. That violates the new public field contract. A later projection can even turn the inherited undercount into an advertised upper bound.

Use the existing internal report-combination entry point. This change concerns report arithmetic only and requires no change to segment lifecycle or policy.

- Build two ReportOnly inputs with the **same** two-contribution policy. Input A finishes observing `{"x":"a"}`. Input B observes `{"stop":{"nested":0}}` and stops before observing its next record `{"x":["b","c"]}`. Assert B has eligible records, is incomplete, and has no row for `x`. An independently unrestricted fresh observer counts `(3,3,3)` for the entire domain. The combined diagnostic can retain observed totals `(1,1,1)` only with `IncompleteObservation`. Check both input orders and a subsequent nonempty projection with unobserved survivors.
- Implement the simplest sound rule on the existing combined report. Any nonempty interrupted contributor that cannot certify absence of the field invalidates exactness and the upper-bound guarantee for that field. A conservative downgrade for all affected fields when such an input is present is acceptable. An exhaustive input missing the path contributes exact zero. A contributor with zero eligible records is neutral. Preserve the existing weaker guarantees from field rows that do exist. Use checked addition for the three totals, as the original spec requires. Test a fully counted control and preserve exact-null/record-count behavior.

[Field accumulation](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/index/segment/consolidation.rs#L98) · [Status calculation](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/index/segment/consolidation.rs#L154) · [Final field status](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/index/segment/consolidation.rs#L281)

## 4. Remove the remaining duplicate work at segment sealing

The compact observation fixes retained memory ownership. Sealing still calls a borrowing `observation()` and then a consuming `finish_candidates()`. The first pass clones each field's empty-selection bitmap, unions ancestor memberships, sorts the fields and clones the entire empty-ancestor map. The second pass sorts fields and unions the same prefixes again. The code also clones distinct-key sets for the compact observation. These operations are visible in the source. Their individual wall-clock costs have not been isolated.

- Sort the owned field vector once and apply ancestor unions once on the builder's working state. Compute compact observation counts while those sets exist. Capture facts needed for global policy before local publication can discard them. Preserve budget and truncation semantics and short-dictionary membership checks.
- Have the consuming finish path return the native candidate state plus an optional compact observation. Move the prefix table into it after the last local use. Move bounded key collections once both consumers have obtained the counts or keys they actually need. Preserve independently owned copies when required by live consumers. When capture is disabled, omit observation construction and accounting.
- Retain the published wrapper's existing ownership and drop behavior, the binary search on sorted compact observations, the bounded construction policy, and the current ownership-transfer optimization. Use the existing semantic-parity and lifetime tests.
- For the final report, run the focused regressions and one `cargo test --workspace --locked`. Only the sealing change warrants another ordinary construction comparison. Three alternating pairs on the same warm eight-worker Musenalm corpus are sufficient for an initial comparison. Record individual pair order and elapsed times; treat overlapping measurements conservatively. A claimed speedup is unnecessary.

[Two finishing calls](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/construction/segment_admission.rs#L962) · [Copied field bitmap](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/construction/inference.rs#L1983) · [Copied prefix table](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/construction/inference.rs#L2106) · [Second sort and ancestor pass](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/src/construction/inference.rs#L1702)

## Measurement and verification boundary

The report records 668 passing core tests, 16 ignored, and a passing workspace suite. The ordinary benchmark calls `Index::start_build` and waits for completion, so it does exercise segmented initial construction. Reported median build times were 891.520 → 896.718 ms with inference disabled and 1,563.542 → 1,555.784 ms with AutoIndex. AutoIndex peak RSS was 914.0 → 904.3 MB. Three overlapping pairs establish no clear construction-speed change. This review checked the implementation, tests, public contracts, and published measurements. The additional failure cases above are source-traced acceptance fixtures. Cargo was unavailable in the review environment.

[Repair report and samples](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/docs/ai/done/objekte_query_proof_facet_resolution_and_inference_repair_specification.md#L251) · [Benchmark entry point](https://github.com/Simon-Martens/objekte/blob/0345baa3e9/crates/core/examples/bench_index_construction.rs#L145)

## Implementation report

Task identifier: `segmented-facet-and-inference-finalization-corrections`

Completion date: 2026-11-20

Base and reviewed report tip: `0345baa3e94d69efbabd6a4235eaf1e3a312481e`

Final code commit: `f253716333`

Branch: `repair/path-posting-query-evaluation`

Worktree: `/home/simon/source/objekte-repair-path-posting-query-evaluation`

The task directed continuation in the existing repair worktree, so no
additional integration worktree was created.

### Facet identity

The reviewed tip returned numeric `7` for `@backlinks.x`; after validation of
`PathRoot::Data`, both segmented and ordinary paths return
`posting values require a logical data field`. The effective binding is now
computed once from explicit request, schema default, then literal `default`,
and is passed through ordinary, finalized-inference and provisional fallback
resolution.

A schema-selected `fold` binding on an unindexed raw `title` field now returns
the lowercase key `hello` with count 2 for both omitted and explicit `fold`
requests. Explicit `default` still rejects the prohibited raw index. Cursor
validation continues to use the selected binding and version.

### Mixed facet family

Two independently aggregated layouts were constructed from numeric `7` and
string `"a"` segments. Both orders expose observed Number and String kinds,
no complete general dictionary, and short-string candidates. Both now return:

- key string `a`, count 1;
- two eligible and two present records;
- one indexed and one unresolved record;
- `value_counts_complete = false` and no continuation.

A selection containing only numeric `7` returns no enumerable keys and one
unresolved record. A provisional local fallback whose first segment is
structural/length-only and whose later segment is enumerable skips the
unsuitable first dictionary. Existing partial, pagination, cursor, numeric
absence and complete-dictionary tests pass.

### Occurrence status and checked addition

An exact `x` contributor combined with a nonempty interrupted contributor that
has no `x` row now produces `IncompleteObservation` in both input orders. The
combined observed totals remain `(1,1,1)` without claiming a survivor bound.
Projection with unobserved survivors preserves the incomplete status. A fresh
unrestricted observation of the complete domain reports exact `(3,3,3)`.

The three occurrence totals now use checked addition during consolidation.
Existing exact-null and record-count assertions remain green.

### One-pass segment sealing

The consuming inference finish now:

- sorts owned field states once;
- applies each empty-ancestor union once;
- captures compact counts, flags and bounded keys before candidate dictionaries
  are consumed;
- moves the prefix table into the optional compact observation;
- omits compact observation allocation when capture is disabled.

The previous borrowing `observation()` pass and the second sort/ancestor pass
were removed. Existing inference parity, compact observation, lifecycle
release, Disabled-inference and construction tests pass.

### Validation

- `cargo test -p objekte-search-core --lib --locked`: 672 passed, 16 ignored.
- `cargo test --workspace --locked --no-fail-fast`: passed across CLI, desktop,
  core, integration tests, and doctests.
- `cargo check --workspace --all-targets --locked`: passed.
- `cargo check -p objekte-search-core --all-targets --features
  detailed-profiling --locked`: passed.
- `cargo clippy --workspace --all-targets --locked`: passed without errors and
  retained existing warning-level diagnostics.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

### Construction comparison

Separate ordinary release binaries used the warm 148,486-record Musenalm ASCII
corpus, eight workers, a disabled verified cache, and three alternating
fresh-process pairs.

| Mode | Baseline samples | Candidate samples | Baseline median | Candidate median |
| --- | --- | --- | ---: | ---: |
| Inference disabled | 885.497, 889.765, 893.226 ms | 897.094, 897.573, 908.957 ms | 889.765 ms | 897.573 ms |
| AutoIndex | 1577.552, 1587.489, 1602.142 ms | 1576.019, 1583.150, 1588.084 ms | 1587.489 ms | 1583.150 ms |

Both distributions overlap and neither change is material. Median peak RSS was
535.4 MB baseline and 540.1 MB candidate without inference, and 868.8 MB
baseline and 862.5 MB candidate with AutoIndex. No speedup claim is made.

The repair branch and worktree are retained for the owner's review and
integration into `master`.
