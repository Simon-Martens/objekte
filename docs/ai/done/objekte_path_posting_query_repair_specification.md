# Objekte path posting indexes and query execution — repair specification

**Disposition:** Hold the merge. A valid filter can return the wrong record IDs, and an inferred facet fallback can index outside the record table. Preserve the path-oriented design and the construction improvement while correcting these and the lifecycle defects below.

**Review target:** `integration/path-posting-query-evaluation` at `b3f7e9dc9c`. The implementation commit is `12e8e048d6`. The comparison baseline is `32992e537e`. This review traced the Rust source and compared it with the original specification. The branch reports 637 passing core tests, 16 ignored, and a Musenalm construction median of 0.688 s versus 0.752 s at baseline. Those are the implementer’s measurements. This review environment had no Rust toolchain. The examples below are regression cases for the implementing agent to execute. [Reviewed branch](https://github.com/Simon-Martens/objekte/tree/b3f7e9dc9c) · [Original assignment and implementation report](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/docs/ai/done/objekte_posting_index_and_query_execution_specification.md)

## 1. Require matching path semantics before using a posting

**Wrong results.** The catalog retrieves a posting by its sequence of field names. Its `declared_array_steps` check establishes that each wildcard follows an array declaration at the current field prefix. It also accepts a wildcard in the final position. A posting for `x` describes the array itself. The source evaluator for `x[*]` selects its elements. The distinction survives binding but is lost when the catalog approves reuse. Once the planner accepts an exact posting, a source check cannot correct either false positives or false negatives. [Catalog eligibility](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/query_access/catalog.rs#L97) · [Wildcard check](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/query_access/catalog.rs#L348) · [Source path traversal](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/query/semantics/record.rs#L98) · [Reference predicate evaluation](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/reference/filter.rs#L45)

Declare `x` as an optional array of nullable strings. Use four logical records, A = `{"x":[]}`, B = `{"x":[null]}`, C = `{"x":["Ada"]}`, D = `{}`. These expectations can be used through the public filter API.

| Filter | Required IDs | IDs implied by the posting for the container |
| --- | --- | --- |
| `x[*]:exists = true` | B, C | A, B, C |
| `x[*]:null = true` | B | none |
| `x[*]:string = true` | C | none |

- Reject reuse of the container posting for a trailing wildcard in every applicable catalog proposal, including candidate and self-comparison shortcuts. Use the existing source evaluator with the incoming restriction. An empty candidate from the wrong posting already loses records.
- Keep the optimized intermediate wildcard case `contributors[*].type` where explicit traversal and the indexed field path reach the same terminal values. Limit the optimization to shapes for which that equivalence is proved. Use fallback for consecutive wildcards until equivalence is established, and retain the existing positional fallback.
- Test true and false predicates, an incoming selective AND, and an OR containing an independently matching record. Compare IDs to the reference evaluator through both initial and delta-segment execution. Keep the existing zero-source-read assertion for Q203. Add the terminal-wildcard fixture separately.

## 2. Restore inferred path admission and make its output pass validation

**Invalid inferred state.** `field_admission_decision` publishes a path on `has_selected_null` alone. This precedes several existing type, shape, and value-budget decisions. For a field observed only as null, the new test expects a built index with kind `Unknown`. The null observation records a slot and returns before registering any observed kind. The final validator requires the reported kinds to include the index kind, with a distinct exception for arrays. This test’s produced state therefore fails the validator with `snapshot inferred posting statistics mismatch`. The test currently stops after checking the builder output. The assignment explicitly preserves existing admission decisions. [Admission](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/construction/inference.rs#L132) · [Null observation](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/construction/inference.rs#L925) · [Validator](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/inference/mod.rs#L414) · [Current regression test](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/inference/tests.rs#L1630)

- Make the original admission decision first, in both per-segment and collection-wide inference. When that decision publishes a path and terminal observations are complete, retain its selected-null entry even if general value coverage is partial. A null-only undeclared path with no independently qualifying capability retains its baseline status and can use source evaluation. Preserve declared source-backed null lookups, including existing coverage with ordinary value indexing disabled.
- Compare the admission inputs with the baseline, including how a null observation contributes to scalar/mixed-shape accounting. Keep the legacy report definitions separate from the selected-null query fact. Preserve explicit opt-out, report-only mode and observation-budget decisions.
- Replace the test that accidentally requires broader admission. Include null-only, missing, admitted nullable numeric, mixed, and budget-limited paths. Assert status, actual capability, and query IDs. Run the production inference validator on built states. On admitted paths, also assert zero source reads for exact `:null`. Retain strict validation of actual observed types.

## 3. Keep segment-local record slots in their owning segment

**Panic or wrong record class in a public facet.** When an active segment lacks a compatible complete inferred posting, `scan_inferred_segment` gathers local slots from that segment, looks up each local record, and filters to logical records. When reading their bytes it uses the same slot number to index `collection.generation.records`. A delta can contain slot 1 when the base generation has only slot 0. Even an in-range base slot can refer to a record of a different class. This fault is in the existing fallback reached by the facet path covered by the assignment. [Fallback selection](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/repository/posting_values.rs#L838) · [Segment-local selection and incorrect class lookup](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/repository/posting_values.rs#L351)

- Pass `RecordClass::Logical` after the existing logical-record filter, or carry the class from the same segment row if the interface requires it. Preserve the segment’s file identity, revision check, byte range, and active-slot mask.
- Exercise the public posting-values operation on a one-record base with an admitted inferred numeric field. Publish a delta of at least two logical records whose field observations do not provide a compatible complete posting. Assert that the public request enters fallback, processes local slot 1, and returns the same values and counts as a fresh build over the active records. Include a retired slot. A checked lookup into the base would merely turn the panic into a spurious error.

## 4. Finalize posting statistics after publication reconciles admission policy

**Inconsistent published index.** Publication can remove ordinary dictionary keys, retain `ScalarKey::Null`, and assign new completeness flags without refreshing statistics. `validate_eligible` compares stored distinct counts to the live dictionary. It also rejects a nonempty lengths map when `length_complete` is false. Published segments can violate either invariant after collection-wide policy narrows a locally admitted capability. [Publication pruning](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/lifecycle/publication.rs#L745) · [Posting validation](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/index/posting/mod.rs#L476)

- Reconcile capabilities and retained maps together. Preserve the terminal-null entry for eligible paths. When the compatible representation requires length keys to be absent, discard them as the capability is withdrawn. A collection-level decision alone cannot confer missing local coverage. Finalize derived statistics once after a material change. Reuse the existing finalizer; preserve independently established selection and facet-equivalence information.
- Force policy reconciliation with two individually admissible segments whose combined observations disable a capability. Use disjoint scalar kinds with a selected null in one segment for dictionary pruning, and a small length-key budget with disjoint keys for length pruning. Check the actual published posting, its advertised coverage and distinct count, and run posting/domain validation. Compare public null, value and length queries with the reference across both segments.

## 5. Preserve the meaning and completeness of inference reports through compaction

**Incorrect diagnostics labeled complete.** Projection assigns `null_occurrence_records` from the selected-null posting. Inference also counts null array elements for this diagnostic. Projection zeroes `empty_selection_records` when no empty traversal is reported, although the builder counts a terminal empty array too. Consolidation separately sets `field.complete = true` whenever it can merge an index, even when a contributing projected report was incomplete. [Builder observations](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/construction/inference.rs#L1456) · [Projection](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/index/segment/projection/inference.rs#L105) · [Consolidation](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/index/segment/consolidation.rs#L217)

- Use retained records `x=[]`, `x=[null]`, `x=[7]` and retire an unrelated fourth record to force projection. A fresh report for the three survivors has one null-occurrence record, zero records with a selected terminal null, and one empty-selection record. A query for `x:null = true` over these survivors returns none.
- Derive exact counts when retained evidence supports them. For example, a complete zero-length posting counts terminal empty arrays. Account for overlap before combining record counts. When the representation cannot reconstruct a diagnostic, use the report’s incomplete convention and keep the query capability independently exact where justified. Document which values in an incomplete report are bounds. An unqualified zero is misleading when the true count may be positive.
- Propagate report completeness across every contributing nonempty shard. A compatible merged posting proves membership for its supported queries only. Test fresh, projected and compacted states and compare all counters marked complete with a fresh scan of the same live records. Exercise a projected incomplete shard merged with a complete one. These diagnostics do not require permanent per-slot structural maps or a corpus reread during compaction.

## 6. Avoid unnecessary file reads for inferred facet presence

**A terminal empty array can trigger reads of every missing record in the requested domain.** Inference currently sets `legacy_present_equivalent` to `field.empty_selection.is_empty()`. Its own report computes legacy presence as terminal presence plus the records in `empty_selection` outside `present`. A terminal `[]` appears in both sets. The facet takes the conservative flag and verifies slots outside terminal presence, even when equality was already provable at construction. [Equivalence flag](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/construction/inference.rs#L1761) · [Report definition](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/construction/inference.rs#L1830) · [Facet verification](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/repository/posting_values.rs#L1380)

- While both builder sets are available and complete, compute equivalence with `field.empty_selection.is_subset(&field.present)`. This uses the existing bitmaps and requires one stored Boolean. Keep restricted source verification when a successful empty traversal really did have no terminal value.
- Test an inferred numeric-array facet on `[]`, `[3]`, `[5]` and many records missing the path. Require the correct public count and zero facet source reads on a complete posting. Add a nested `items.name` control with `items=[]` and other records that admit the path. Check the legacy count and prove that any necessary verification respects the requested domain.

## 7. Cost the physical dictionary operation the executor performs

**Planner underestimates scans.** A nullable, multivalued string path can require unioning every string key. The executor checks multiplicity before using the cheap presence-minus-null shortcut. The estimator can still charge one lookup for that path. The general nonempty union and structural candidate union can also be priced as constant. Separately, `without_null` clones the entire presence bitmap before intersecting it with an already selective input. [Execution paths](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/index/posting/lookup.rs#L168) · [Cost estimate](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/index/posting/lookup.rs#L378) · [Candidate estimate](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/query_access/catalog.rs#L128) · [Restricted subtraction](https://github.com/Simon-Martens/objekte/blob/b3f7e9dc9c/crates/core/src/index/posting/lookup.rs#L1077)

- Share the decision about the physical operation between estimation and execution, keeping it private to posting access. Distinguish a direct bitmap, restricted subtraction, typed value range, length range, combined scan and presence-only candidate as needed. Use existing bounded probes or map size. Limit estimator work by the existing 33-key cap and price broad scans accordingly. Estimation must never build the union.
- For restricted subtraction, form the intersection of the borrowed input and presence first, then subtract the borrowed null posting. Keep the full-domain path when no restriction was supplied.
- Use many distinct strings across repeated terminal occurrences, including null and a string selected in the same record. Assert that a typed range costs more than a one-posting proof, planner probe counts stay bounded, and restricted output matches the reference. Check the candidate and nonempty union paths too. Use deterministic counters for these assertions.

## Delivery and verification

1. Work on the reviewed integration branch or a repair branch based on it, in its own worktree. Leave the final merge to the user. Keep the existing `PostingIndex` design, public query syntax, and per-segment ownership.
2. Land the two public correctness regressions first (§§1 and 3). Capture the failure against the reviewed revision and the passing ID/count assertions after the fixes. Then repair admission, publication, diagnostic completeness and avoidable query work.
3. Run `cargo fmt --all -- --check`, `cargo check --workspace --all-targets` and `cargo test --workspace`. Run the existing release structural-reference example for Q203 and the other affected null/type cases. Record result equality and source-I/O counters for both covered and fallback paths.
4. Repeat the branch’s three-sample fresh ASCII construction measurement under the same worker, corpus and cache settings. Record individual samples and median. A focused high-cardinality type query and sparse facet fixture are sufficient for the new performance cases. Report observed times separately from correctness and I/O assertions.
5. In the integration report identify the exact tested revision, commands, fixture results and any remaining fallback. A fallback that verifies only the surviving candidates is an acceptable outcome wherever the retained index cannot prove the predicate.

## Implementation report

- Task identifier: `path-posting-query-evaluation-repair`
- Completion date: 2026-11-20
- Review base: `b3f7e9dc9c04fd4c83856ff1b551c4d3f22a1f20`
- Repair branch: `repair/path-posting-query-evaluation`
- Repair worktree: `/home/simon/source/objekte-repair-path-posting-query-evaluation`

### Implemented repairs

- Posting reuse now requires an equivalent terminal selection. A trailing
  wildcard, consecutive wildcards, and positional selectors cannot reuse the
  container field posting. Intermediate declared array traversal such as
  `contributors[*].type` remains optimized. Public initial and delta-segment
  tests cover positive/negative state predicates, selective AND, and OR.
- Segment-local inferred facet fallback now evaluates records as
  `RecordClass::Logical` after applying the segment's local active mask. The
  regression fixture uses a one-record base, a multi-record delta, local slot
  1, and a retired delta slot.
- Inference admission was restored to the original decision order. Null-only
  undeclared paths remain source-evaluated, while selected null is retained on
  paths that already qualify through numeric, string, length, or other existing
  admission evidence. The production inference validator is exercised on null,
  missing, admitted nullable numeric, mixed, and budget-limited paths.
- Publication now clears lengths when the collection-level decision withdraws
  length completeness, retains selected null, and recomputes
  `PostingIndex::finish_stats()` once after reconciliation. Global disjoint
  length-key pressure also withdraws the collection-wide length capability.
  Regression fixtures validate published dictionaries, advertised capabilities,
  statistics, and public query parity.
- Projected inference reports retain the builder meanings for null occurrences
  and empty selections where evidence permits, derive terminal empty arrays from
  complete zero-length postings, and use conservative upper bounds with
  `complete = false` where per-slot reconstruction is impossible. Consolidation
  propagates contributor incompleteness instead of labeling the merged field
  complete.
- Inferred facet legacy presence now uses
  `empty_selection.is_subset(present)`. Terminal empty arrays therefore keep the
  cheap presence equivalence, while a genuine successful empty traversal still
  triggers domain-restricted verification. The regression removes source files
  after construction to prove the complete array facet is file-free.
- Exact state execution and cost estimation share physical-operation
  classification for direct presence subtraction, typed value unions, length
  ranges, combined nonempty scans, and candidate unions. Planning remains
  bounded to 33 probed keys and never materializes a union. Restricted
  subtraction now intersects the borrowed input with presence before removing
  null.

### Contributing commits

- `fc05cc0342` — Fix wildcard and delta facet correctness
- `dd5b97de91` — Restore baseline inference admission decisions
- `df77b7ae7e` — Reconcile publication and projected inference reports
- `c441d43d6e` — Align posting costs with physical state execution

### Validation

- `cargo fmt --all -- --check` — passed.
- `cargo check --workspace --all-targets` — passed.
- `cargo test --workspace` — passed. Core: 644 passed, 16 ignored; all CLI,
  desktop, integration, and documentation tests passed.
- Release `verify_structural_reference` was run for Q106, Q107, Q116, Q117,
  Q120, Q195, Q196, Q198, Q203, Q207, Q208, Q218, Q219, and Q220. Counts and
  page SHA-256 digests match the reviewed revision and source reference. Every
  case reports zero files opened, bytes read, record parses, and scanned
  records.
- Synthetic tests prove zero source reads for exact covered null/type
  predicates, terminal-array public facets, and selected domains that exclude
  genuinely ambiguous empty traversals. A full-domain nested diagnostic that
  cannot be reconstructed correctly returns an unavailable error after its
  source files are removed.

### Performance evidence

Three fresh ASCII Musenalm processes were run for each revision with four query
workers, verified cache disabled, and Q198/Q203/Q207/Q220:

| Metric | Reviewed `b3f7e9dc9c` | Repair `c441d43d6e` |
| --- | ---: | ---: |
| Build samples | 0.721, 0.715, 0.689 s | 0.734, 0.673, 0.708 s |
| Median build | 0.715 s | 0.708 s |
| Median resident bytes | 646,553,600 | 646,963,200 |
| Median peak resident bytes | 696,340,480 | 695,508,992 |
| Estimated index bytes | 104,345,704 | 104,345,704 |

All builds parsed 148,486 records. Median query times were:

| Case | Reviewed median | Repair median |
| --- | ---: | ---: |
| Q198 `year:number = true` | 94.114 µs | 94.565 µs |
| Q203 `contributors[*].type:null = true` | 85.507 µs | 85.301 µs |
| Q207 `contributors.type:string = true` | 238.962 µs | 241.374 µs |
| Q220 `contributors.type:string = false` | 251.317 µs | 253.339 µs |

Build, memory, and query changes are within observed run-to-run variation. The
focused deterministic posting-cost fixtures verify that a 100-key multivalued
typed union costs more than the one-posting proof on a single-valued field,
planner probes stay at or below 33 keys, candidate probes stay bounded, and
restricted execution returns exactly the requested intersection.

### Remaining fallback

- Trailing and consecutive wildcards use the source evaluator with the incoming
  restriction. They no longer use the container posting.
- Mixed typed or partially covered dictionaries may still require verification
  of surviving candidates when the retained dictionary cannot prove the type or
  content predicate.
- Projected inference reports whose per-slot diagnostic facts cannot be
  reconstructed are explicitly incomplete and expose conservative bounds.
  Posting query membership remains independently exact where its summary proves
  the predicate.

No unresolved in-scope defects are known. No subordinate worktrees were created;
the repair branch and worktree are retained for user review.
