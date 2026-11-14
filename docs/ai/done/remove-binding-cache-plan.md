# Agent specification: remove the prepared-query binding cache

Task: `remove-binding-cache`
Repository: `Simon-Martens/objekte`
Plan date: 14 November 2026
Base revision: current `integration/hybrid-index-storage` head `dd3f8feb19` (or the merged master equivalent)

## 1. Objective

Delete the repository-local prepared-query binding cache completely. Every query will be bound fresh. Keep the binding logic (`BoundExpr`/`BoundFts`), but remove the cache, its accounting, its options, its metrics, and its benchmark surface.

The change is justified by a dirty release measurement on the current branch:

- 194-query suite: **1.518 ms total**, average **7.82 µs**, max 210 µs.
- 220-query extended suite: **1.438 ms total**, average **6.54 µs**, median 3.2 µs, p90 8.8 µs, p95 15.6 µs, max 204 µs.
- The tail is regex FTS compilation: `Almanach.*` ~183 µs, `Almanach|.*` ~204 µs, `Taschenbuch.*` ~51 µs, `(?m)^Almanach` ~30 µs.
- Ordinary typed filters, relations, sorts and metadata predicates are ~1–10 µs.

Binding is negligible relative to search. The cache saves at most ~1.5 ms across an entire 200-query campaign. A dedicated regex cache can be considered later if repeated regex compilation ever matters; this task removes the general binding cache.

Non-goals:

- Do not change query semantics, binding rules, planner behavior, index formats, or storage.
- Do not add a result/membership cache. That is a separate future task.
- Do not add a regex-specific cache.
- Do not rewrite historical `docs/ai/done/` reports or archived benchmark results that mention the old cache.
- Do not merge into `master`.

## 2. Base, branch and workflow

Record the actual base at start. The reviewed working checkout is on `integration/hybrid-index-storage` at `dd3f8feb19`; if that branch has been merged into master, base on the merge commit.

- New task branch: `integration/remove-binding-cache`.
- New worktree: `/home/simon/source/objekte-integration-remove-binding-cache`, sibling of the original checkout.
- Keep `/home/simon/source/objekte` on its existing branch; do not reuse the user's dirty checkout for implementation.
- Read `AGENTS.md` and `docs/ai/README.md` first.
- Push immediately after every commit. Do not force-push or modify remotes.
- Run `cargo fmt --all`, `python3 scripts/format-rust-spacing.py`, workspace tests and Clippy before each push. Report existing repository-wide formatting drift rather than committing a global reformat.

The user's current checkout contains an untracked `examples/musenalm/.objekte/indexes/` store. Leave it untouched.

## 3. Current implementation map

| Area | File | Current behavior |
| --- | --- | --- |
| Cache and binding | `crates/core/src/prepared.rs` | `PreparedCache` LRU keyed by bincode of `(collections, filter, fts)`; `prepare` returns `(PreparedQuery, hit)` |
| Cache tests | `crates/core/src/prepared.test.rs` | 249 lines of cache accounting/eviction/regex-budget tests |
| Repository ownership | `crates/core/src/repository.rs` | `pub(crate) prepared: Mutex<PreparedCache>`; `RepositoryOptions.prepared_cache_bytes` (16 MiB default) |
| Repository construction | `crates/core/src/repository.rs`, `crates/core/src/storage.rs` | Two `prepared: Mutex::new(...)` initializers |
| Query call sites | `crates/core/src/lifecycle.rs`, `crates/core/src/run.rs` | Destructure `(prepared, hit)` or `(_, plan_hit)` |
| Hit counter | `crates/core/src/measurement.rs` | `WorkCounters.prepared_plan_hit` |
| Counter plumbing | `crates/core/src/run.rs` | `prepared_plan_hit` parameters threaded through order/page helpers; `counters.prepared_plan_hit = plan_hit` |
| Test expectation | `crates/core/src/run.test.rs` | Asserts `prepared_plan_hit` behavior |
| Benchmark harness | `crates/core/examples/bench_musenalm.rs` | `--prepared-cache-bytes`, manifest field, CSV `prepared_plan_hits`, report column |
| Construction bench | `crates/core/examples/bench_index_construction.rs` | Writes `prepared_cache_bytes` in manifests |
| Docs | `docs/search-core.md`, `bench/README.md` | Describe the 16 MiB prepared budget and prepared-plan hits |

Search commands for the implementer:

```
git grep -n 'prepared_cache_bytes\|prepared_plan_hit\|PreparedCache\|prepared::prepare\|bound_expr_retained_heap_bytes\|bound_fts_retained_heap_bytes'
```

## 4. Required changes

### 4.1 `crates/core/src/prepared.rs`

Replace the cache with a pure binding function:

```rust
/// Bind one query against the selected collection generations.
/// Binding is performed for every request; there is no repository cache.
pub(crate) fn prepare(repo: &Repository, query: &Query) -> Result<PreparedQuery, String>
```

Remove:

- `PreparedCache`, `Entry`, `PreparedCache::new/get/insert/entry_bytes/used_bytes`;
- `PreparedQuery::retained_heap_bytes`;
- the bincode cache key construction and the `repo.prepared` lock/insert;
- the `#[cfg(test)] #[path = "prepared.test.rs"] mod tests;` block and delete `prepared.test.rs`.

Keep:

- `PreparedQuery { collections: Vec<(usize, Option<BoundExpr>, Option<BoundFts>)> }`;
- per-collection `bind_filter_with_relations` / `bind_fts_with_relations`;
- parameter type tracking and the `unknown or unloaded collection scope` error.

Update the module doc comment to say binding is fresh per request, not cached.

If `bound_expr_retained_heap_bytes`, `bound_fts_retained_heap_bytes`, `BigramRequest::retained_heap_bytes`, or `RegexCondition`/`CompiledRegex::retained_heap_bytes` become unused after the cache removal, prune the now-dead chain or leave clearly test-only helpers only if a test still uses them. Do not add new public APIs.

### 4.2 `crates/core/src/repository.rs` and `storage.rs`

- Remove `pub(crate) prepared: Mutex<crate::prepared::PreparedCache>` from `Repository`.
- Remove both `prepared: Mutex::new(...)` initializers (`repository.rs` and `storage.rs`).
- Remove `RepositoryOptions.prepared_cache_bytes` and its 16 MiB default.
- Remove `exported_default_prepared_cache_budget_is_sixteen_mib` from `repository.test.rs`.
- Remove `prepared_plan_hit: false` from the counters literal in `repository.rs`.
- Update every in-repo `RepositoryOptions { ... }` construction that names `prepared_cache_bytes` (bench harness, tests) and every struct literal that no longer has the field.

Note: removing `RepositoryOptions.prepared_cache_bytes` is a deliberate breaking API change. The repository is pre-1.0; the option only controlled the deleted cache and must not remain as a misleading no-op. State this in the implementation report. If the user later requires a compatibility shim, add a separate deprecation task.

### 4.3 Remove the hit metric and plumbing

- Remove `WorkCounters.prepared_plan_hit` from `crates/core/src/measurement.rs`.
- Remove `counters.prepared_plan_hit = plan_hit` and all `prepared_plan_hit` parameters/arguments threaded through `run.rs` helpers (`try_inferred_order_page`, `try_stored_order_page`, related order-page functions, and their callers).
- Remove the second tuple element from every `prepare` call site:
  - `crates/core/src/lifecycle.rs:3477`
  - `crates/core/src/run.rs` around 3060, 3135, 3932, 7036, 7064, 7468, 9243, 9990, 10242, 10315, 10457, 10880
- For paths that only used the hit flag (`let (_, plan_hit) = ...`), either bind and use the prepared value where needed or remove the binding if the path does not need it. Do not leave an unused result.
- Update `run.test.rs` to remove the `prepared_plan_hit` assertion and use a semantic assertion instead where the test still matters (for example, verified-result-cache behavior or result shape).
- Audit for duplicate binding within one request (for example, a fallback path that re-plans after an error). Reuse the already-bound `PreparedQuery` where straightforward; do not redesign the fallback.

### 4.4 Benchmark harness and scripts

`crates/core/examples/bench_musenalm.rs`:

- Remove the `--prepared-cache-bytes` argument and its parse branch.
- Remove `prepared_cache_bytes` from both `RepositoryOptions` constructions.
- Remove `prepared_cache_bytes` from the run manifest JSON.
- Remove `prepared_plan_hits` from the CSV header and row, and remove the `prepared_hits` computation.
- Remove the `Prepared hits` column from the Markdown report and its format arguments.
- Keep `verified_result_hit` and its column; the verified cache is not part of this task.

`crates/core/examples/bench_index_construction.rs`: remove `prepared_cache_bytes` from both manifest JSON outputs.

Update `bench/README.md`: remove the paragraph recommending `--prepared-cache-bytes 0` and the claim that the setting is recorded.

Search `bench/*.py`, `scripts/`, and CI/command documentation for the removed flag. The current checkout has no Python references, but re-check before finishing. If a script passes the flag, remove it.

Do not edit archived `bench/results/**` files or historical `docs/ai/done/**` reports. They describe past runs that used the cache.

### 4.5 Documentation

- `docs/search-core.md`:
  - remove “The repository owns prepared and verified caches” and say it owns the verified-result cache only;
  - remove the prepared 16 MiB retention-budget paragraph;
  - remove “prepared-plan and verified-result hits” from benchmark reporting, or update to verified-only.
- `docs/development/hybrid_index_storage.md`: remove any prepared-cache mention if present.
- `docs/cli/obj.md`: no flag exists there, but confirm no stale mention.
- Add a short note wherever query caching is documented: binding is fresh per request; only the verified-result cache and internal decoded-posting cache exist.

### 4.6 Tests

Delete the cache tests. Add or retain only tests that describe binding behavior, not cache behavior. Minimum validation:

1. Repeated `prepare` calls with the same query produce equivalent bound structures and do not share mutable state.
2. A query with a regex FTS compiles once per bind call; do not assert cache hits.
3. Invalid filters still produce the same binding error.
4. Multi-collection and parameterized queries still bind correctly.
5. `RepositoryOptions` construction without the removed field compiles across the workspace.
6. `WorkCounters` serialization no longer contains `prepared_plan_hit`; readers that ignore unknown fields still parse old archived JSON.

Run:

```
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked
cargo fmt --all -- --check
python3 scripts/format-rust-spacing.py --check
```

## 5. Correctness and measurement

### Correctness

Run the existing 220-case extended Musenalm campaign in ASCII and Unicode, five repetitions, with the verified-result cache disabled. All result digests, counts, ordering and evidence must match the base revision exactly.

Focused cases: Q163/Q164/Q173/Q182/Q186 (regex FTS; these lose compiled-regex reuse) and a sample of typed, relation, structural and sort queries. Confirm no failures and no unexplained result changes.

### Performance

The expected effect is:

- binding cost per request becomes visible again, but is ~6.5 µs average and ~200 µs worst case for regex FTS;
- the 220-query campaign may spend roughly an extra 1–2 ms total, dominated by regex compilation;
- memory drops by up to the 16 MiB cache budget plus map/mutex overhead (actual retained depends on use);
- no construction/open change.

Measure:

- per-query binding time for the 194/220 sets using a temporary harness or the existing dirty method; record total, average, p50/p95/max and regex outliers;
- full campaign elapsed/geomean and result digests;
- retained RSS after open/search, and `--diagnostic` output to confirm the removed counter/fields;
- build/flush and open unchanged.

If regex compilation causes a material regression in repeated-query workloads, report the measured cases and recommend a separate regex-only cache. Do not reintroduce a binding cache.

## 6. Commit sequence

1. **Core cache removal** — `prepared.rs`, `prepared.test.rs`, `Repository`, `RepositoryOptions`, `storage.rs`; update call sites and counters. Workspace builds and tests pass.
2. **Metrics/test cleanup** — remove `prepared_plan_hit` from `WorkCounters`, run plumbing and tests; update `run.test.rs`.
3. **Benchmark harness** — remove flag/manifest/CSV/report fields; update `bench/README.md` and any scripts.
4. **Docs** — `docs/search-core.md`, `docs/development/hybrid_index_storage.md`, any active caching docs.
5. **Validation and report** — full campaign, RSS/binding measurements, report under `docs/ai/generated/` or `bench/results/` as appropriate. Keep the integration branch/worktree for review.

Each commit pushed immediately.

## 7. Acceptance criteria

- No production, example, test or active-doc references to `PreparedCache`, `prepared_cache_bytes`, `prepared_plan_hit`, `prepared_plan_hits`, or a prepared-hit benchmark column remain. Archived results and historical `docs/ai/done/` reports are exempt.
- `prepare` performs binding only; no repository cache, no mutex, no bincode key.
- All in-repo `RepositoryOptions` callers compile without `prepared_cache_bytes`.
- Workspace tests and Clippy pass; result digests match the base.
- The report states the measured binding cost, the expected regex outlier, memory delta, and that no binding cache remains.
- No changes to query semantics, storage format, verified-result cache, or source data.
- `master` is untouched; the integration branch is ready for the user's merge.

## 8. Report

Write `docs/ai/generated/remove_binding_cache_results.md` with:

- base and final revisions, branch/worktree, commits;
- what was removed and what remains;
- binding timing distribution (194/220 suites) and regex outliers;
- full campaign commands, result digest parity, failures;
- RSS/memory delta and any measured query-time effect;
- API break note for `RepositoryOptions.prepared_cache_bytes`;
- remaining limitations and whether a regex-only cache is recommended;
- confirmation that archived results/historical reports were not rewritten.

Move the task document to `docs/ai/done/` with the implementation report only after the branch is validated and pushed, per `docs/ai/README.md`.

## Implementation report

- **Task and branch:** `remove-binding-cache`, completed 2026-11-14 on `integration/remove-binding-cache`, based on `0c63651458` from the direct snapshot integration branch. Worktree: `/home/simon/source/objekte-integration-remove-binding-cache`. No subtasks were delegated.
- **Implementation commits:** `1e22afd17b` removes the cache, option, hit counter, dead retained-size accounting, benchmark fields, and active documentation; `edb1e753db` adds feature-gated per-bind timing and campaign runners; `560d836023` archives the raw results and [full report](../generated/remove_binding_cache_results.md). The intentional API break removes `RepositoryOptions.prepared_cache_bytes`. Verified-result and internal decoded-posting caches remain.
- **Validation:** `CARGO_TARGET_DIR=/home/simon/source/objekte-integration-hybrid-index-storage/target/direct-snapshot-access/integration-build cargo test --workspace --locked` passed. `cargo clippy --workspace --all-targets --locked` passed with existing repository warnings. `cargo fmt --all -- --check` and `python3 scripts/format-rust-spacing.py --check` reported broad existing formatting drift in untouched files; no global formatting changes were committed. Current production, examples, tests and active documentation have no cache type, removed option, prepared-hit metric or benchmark column. New tests cover fresh independent binding and reading old archived counter JSON.
- **Benchmarks:** The [raw archive](../../../bench/results/remove-binding-cache/final-20261114/README.md) contains the exact commands, release binary hashes, five-repetition 220-case ASCII/Unicode campaign in memory and on-demand modes, per-case results, binding traces for the current 150- and 220-case manifests, and three paired flush trials. All result digests and source-free managed opens matched. Geometric mean median ratios were 0.985 in memory and 1.021 on-demand; p95 ratios were 0.949 and 0.959. The 220-case trace measured 3,520 binding calls at mean 3.850 µs and median 1.398 µs; regex calls had a 146.219 µs maximum. Peak RSS varied across modes, and identical stored bytes plus no changed flush code support no persistent-size change.
- **Limits and cleanup:** The historical 194-case dirty timing in the specification was not rerun because the current standard manifest has 150 cases. Feature-gated traces include diagnostic binding calls outside latency samples; the report separates these from ordinary release acceptance timings. No regex-only cache is recommended on this evidence. Archived benchmarks and historical done reports were unchanged. After this completion record is pushed, the preserved untracked task source and temporary task memory will be removed. The integration branch/worktree remain for review; master remains untouched.
