# Completion specification: native B-tree, memory-only restoration

## Objective and authorization

Complete the existing restoration on **`integration/restore-native-btrees-memory-only`**.

Reviewed head: `6184a89e1954f80bd7a489288a9b9bb6bd08e241`.

This authorizes implementation of the fixes and completion of the acceptance work below. Continue on the existing integration branch and its worktree. Follow `AGENTS.md` and `docs/ai/README.md`. Preserve unrelated work and follow the repository's commit/push discipline. Do not merge into `master`.

The controlling specifications remain:

- `docs/ai/done/restore-native-btrees-memory-only.md`
- `docs/ai/done/restore-native-btrees-handoff.md`
- `docs/ai/done/restore-native-btrees-commit-ledger.md`

This specification supplements them and identifies the outstanding work. The branch is **not yet accepted as complete**.

## 1. Preserve the implemented architecture

Keep:

- Native `BTreeMap` scalar, length and structural postings.
- Native `HashMap` grams and owned segment/catalog metadata.
- Complete removal of disk-index persistence and serialized-in-RAM readers.
- Fresh prepared-query binding; do not restore its removed cache.
- Verified-result, inferred-order and relation-target caches.
- Progressive opening, cancellation, quiescence, pinned views and unaffected-shard reuse.
- Protocol 4 and ordinary preferences, configuration identity, exports and source checks.

Fix the outstanding problems through targeted changes.

## 2. Eliminate full-index work from GUI status polling

### Finding

In `crates/desktop/src/controller/runtime.rs`, `update_index_ui` calls `view.estimated_index_bytes()` before its unchanged-status guard.

`tick` reaches this through approximately 100-ms polling on the GUI thread. `IndexView::estimated_index_bytes()` currently traverses gram/posting dictionaries and sort keys across the index.

### Required behavior

- Repeated status polls over an unchanged publication must not traverse index contents.
- The estimate must remain correct for its documented meaning through initial construction, publication, mutation, compaction, rebuild and repository replacement.
- Static-base and lifecycle contributions must be accounted for consistently.
- The displayed figure must remain explicitly an estimate, not an allocator/RSS measurement.

### Implementation requirements

Use cached estimates associated with immutable index ownership. Recompute them when the corresponding native contribution is finalized or replaced, and retain them when its `Arc` is reused.

Aggregation over cached collection/segment values is acceptable. Dictionary traversal, record scans and sort-key scans on the polling path are not.

Existing `SegmentAccounting` may be reused where its meaning matches the displayed estimate. Do not silently substitute a differently defined measurement.

### Verification

- Add meaningful regression coverage proving unchanged polling does not repeat index-content estimation.
- Verify publication changes update the displayed estimate.
- Exercise the native GUI with a representative nontrivial corpus, including idle operation with the Index panel closed and open.
- Record corpus, measurement method and before/after observations. Avoid timing-threshold unit tests.

## 3. Make generated-namespace exclusions consistent

### Findings

The directory walkers prune `.objekte` and `objekte-indexes`, but equivalent checks are missing from:

- `selects_file_lexical`
- `PreparedScanSelection`
- Some source-root and canonical-target validation

Consequently:

1. Explicit mutations can admit files omitted by discovery.
2. Selecting a generated directory directly as a collection root bypasses walker pruning.
3. `FollowWithinRoot` can follow an ordinary-named alias into a generated namespace.

Relevant files:

- `crates/core/src/discovery.rs`
- `crates/core/src/repository_configuration.rs`
- Mutation and reconciliation callers of their selection helpers

### Required behavior

Apply the same generated-directory eligibility rule to:

- Initial discovery and streaming discovery.
- Reconciliation and prepared scan selection.
- Explicit add, update and move preparation.
- Explicit collection source-root selection.
- Supported symlink-following paths.

Within the repository, a collection source selection that is itself or lies beneath a reserved generated directory must fail with a contextual source-selection error.

Examples requiring coverage:

```text
data/nested/objekte-indexes/poison.json
.objekte/.indexes/poison.json
.objekte/indexes/poison.json
cache/objekte-indexes/poison.json
```

An ordinary-named alias targeting one of these generated locations must not expose its contents.

### Constraints

- Preserve `.objekte/collections` access for explicit schema/configuration loading.
- Preserve legitimate siblings beneath custom-base parent directories.
- Do not reject ordinary source files merely because their extension is `.idx`.
- Do not change definition ordering, exclusion ordering or live configuration hashes.
- Scope namespace checks to repository/source paths; unrelated ancestors outside the repository must not cause exclusions.
- Reject obvious lexical generated selections before traversing them. Resolve aliases only as needed to determine eligibility; do not enumerate or inspect generated contents.
- Preserve existing recursion, hidden-file, glob and symlink semantics for legitimate source files.

### Verification

Extend existing discovery/lifecycle tests where practical. Cover:

- Recursive and nonrecursive discovery.
- Hidden-enabled discovery.
- Direct selection versus full-discovery parity.
- Explicit add/move/update handling.
- Reconciliation consistency.
- Reserved source roots and descendants.
- Directory and file aliases under `FollowWithinRoot`.
- Legitimate custom-base siblings and configured schema loading.

Tests must assert membership and operation outcomes, not merely successful opening.

## 4. Close the original focused regression gaps

Audit the original handoff's "NEW acceptance cases." Cite existing tests where they genuinely cover the requirement; add focused tests for remaining gaps.

In particular, establish:

- **Metadata guard parity:** supported timestamp comparisons and unsupported transform/period/matcher combinations agree with an independent reference. Supported metadata-only paths retain zero source reads.
- **Native scheduling boundary:** record ranges of **65,536 and 65,537 bytes** preserve the intended scheduling decision, results, single-/multi-worker equivalence and cancellation.
- **Native accounting:** ordinary shards have nonzero estimates; additional postings increase the estimate; active-mask-only retirement retains physical ownership accounting; compaction/remapping refresh it; hysteresis works without source reads.
- **Frontend compatibility:** removed flags fail, protocol-4 queries succeed, removed storage actions remain absent, and old preference fields do not discard ordinary settings.

Do not weaken assertions, ignore failures or substitute implementation-derived expected results for independent checks.

## 5. Complete acceptance validation and retain evidence

### Workspace checks

Run against the final combined code:

```sh
cargo test --workspace --locked --no-fail-fast
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
git diff --check
```

Follow repository formatting policy without bundling unrelated formatting.

Compare Clippy diagnostics with the baseline by diagnostic/location, accounting for deleted code. **A lower total warning count does not prove there are no new warnings.** Fix introduced warnings or document their disposition accurately.

### Query and performance gates

Acceptance baseline:

```text
START = 66b932f31f922a13062a61658e843e3033b3aff4
```

Complete the original handoff's matched campaigns:

- Extended **220 cases × ASCII/Unicode × five repetitions**, with verified-result reuse disabled.
- Full case/profile coverage and comparison of statuses, counts, ordered results and available evidence/digests.
- Independent jq oracle: **150 predicates per alphabet**.
- Applicable fresh-build, readiness, mutation, verified-update, compaction and pinned-view/memory measurements.
- Investigation of material paired regressions.

Recover previous raw evidence where available and valid. Clearly identify its exact code revision. Rerun checks affected by the completion changes; do not present earlier runs as results from later code.

Retain a durable evidence package in the repository's established benchmark/generated-report locations, containing:

- Exact resolved commands and working directories.
- Code revisions and benchmark binary hashes.
- Corpus identity and relevant environment/options.
- Raw result tables, digests and comparison summaries.
- Relevant frontend and tracing logs.
- Explanations of omissions and retained limitations.

Temporary paths alone are insufficient. Large binaries and source corpora need not be committed.

### Actual frontend matrix

Exercise the actual CLI, PTY TUI and native GUI using maintained tooling.

Cover applicable:

- Source open and progressive availability.
- Query, completion, count and paging.
- Source changes/checks.
- Rebuild and compaction.
- Cancellation.
- Repository replacement, including cancelled replacement.
- Close/shutdown.
- Menus and panels showing storage controls remain absent.

Record concrete scenarios and observed outcomes. Backend unit tests supplement this evidence.

### No-index-I/O matrix

Combine filesystem inventories with focused tracing across:

1. Source open.
2. Query.
3. Mutation.
4. Compaction/rebuild.
5. Shutdown.

Use disposable fixtures containing absent, malformed and inaccessible old artifacts, including both historical `.objekte` index spellings, legacy snapshots and `objekte-indexes` directories. Include canonical-alias cases.

Verify:

- No old-index probing, content reads, writes, renames, deletion or spill.
- Artifact contents and metadata remain unchanged.
- Logical source results remain correct.

Distinguish legitimate source/schema reads, directory enumeration, preferences, exports and diagnostic output. Permission-denied coverage must use an appropriately unprivileged process or be reported as unavailable.

## 6. Complete the preservation ledger

The existing ledger covers only:

```text
B = da8fde68472b8646a63a05b3b3654c58de877301
H = a0a0691897c6ff24985fc4db9ae51f5d77a89899
```

Append the missing **H..START** disposition, including the TOML work and merge:

```text
b31db7b2eb97
f4337dc5c2d0
e6f5fe8f25b6
7e32a5a7674e
31d0f3c6a23e
66b932f31f92
```

Verify this range from Git history rather than relying solely on this list. Inspect merge resolutions and document which behavior is removed, retained or adapted.

Preserve the original ledger and historical reports. Update the K01–K29 evidence map with concrete implementation/test/evidence references.

## 7. Completion report and delivery

Append a clearly dated **review-completion addendum** to:

```text
docs/ai/done/restore-native-btrees-memory-only.md
```

Record:

- Reviewed head and completion implementation commits.
- Each finding, its fix and verification.
- Focused-test coverage.
- Exact validation commands and outcomes.
- Durable benchmark/frontend/tracing evidence.
- H..START ledger completion.
- Warning comparison and any remaining limitations.

Explicitly correct or supersede earlier unsupported completion claims.

Commit and push on the existing integration branch following repository rules. Retain its integration worktree.

### Acceptance condition

The branch is ready only when:

- GUI polling performs no index-content traversal.
- Generated-namespace selection is consistent across all relevant entry points and aliases.
- Required regression, frontend and no-index-I/O gates are satisfied.
- Performance and preservation claims have auditable evidence.
- The updated report and implementation are pushed.

An unresolved in-scope failure or unavailable mandatory gate must be reported as outstanding, not converted into a passing completion claim.

## Implementation report

Completed on 2026-11-14 on `integration/restore-native-btrees-memory-only`.
Reviewed head `6184a89e1954f80bd7a489288a9b9bb6bd08e241`; implementation commit
`2a2c12189d5a4a48ff644d6cf744c6a837b7d3d5`; ledger appendix
`4078a3c5d65446b413ac343a2701316e3c66097a`; evidence/report commit
`44ff8b5f62bcf197d62af46d9e01c2e3896f05c2`; external-review correction
`7ec29746c81cae8bab958c9688c8008fdd5d791f` (canonical source-root rejection
and prune-before-canonicalize for reserved links); final evidence refresh
`850f69822284b44418e9059278b8a229c4fafcba`. Final evidence and reruns
are recorded against `7ec29746c8`. The detailed finding/fix/verification
record is the Review-completion addendum in
`docs/ai/done/restore-native-btrees-memory-only.md`; durable evidence is under
`bench/results/restore-native-btrees-memory-only/`.
