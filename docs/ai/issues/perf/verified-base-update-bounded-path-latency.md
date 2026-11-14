# Verified base update remains above the single-digit-millisecond target

- **Category:** perf
- **Originating task:** `index-lifecycle-repair`; separated from the resolved collection-sized rewrite issue while sorting open issues in `remove-legacy-snapshot-stack`.
- **Observed branch/commit:** `integration/index-lifecycle-repair`, validation reported in `docs/ai/generated/index-lifecycle-repair/validation.md`.
- **Code:** `crates/core/src/lifecycle_verified.rs`, `crates/core/src/lifecycle_compaction.rs`.

## Observation and impact

The repaired slot-preserving path no longer rewrites an entire collection, but three independent Musenalm stress runs measured original-base verified update at a 45.585 ms median (44.925–45.840 ms), above the task's single-digit-millisecond engineering target. Profiling attributed roughly 15–16 ms to affected-shard compaction, 3–4 ms to consolidation, and 12–13 ms to dense remapping. Interactive callers that choose `UpdateStrategy::PreferVerified` still incur this latency; ordinary appended updates remain the default and were measured at 0.561 ms median in the same report.

## Reproduction and expected behavior

Run the committed `bench_lifecycle_mutations` example in `stress` mode on a fresh disposable Musenalm copy with `OBJEKTE_BENCH_MUTATE_COPY=1`. See `docs/ai/generated/index-lifecycle-repair/validation.md` and its raw `admission-final-{1,2,3}.tsv` measurements. The engineering target is a single-digit-millisecond verified original-base update while retaining physical removal, pinned-view safety, and bounded memory.

## Scope and possible approaches

This cost is outside the `remove-legacy-snapshot-stack` task, which deletes an unrelated persistence codec. The measured phase breakdown identifies local work worth investigating. A smaller affected-shard compaction or less dense remapping might reduce it, but neither is validated here; both need correctness and fresh-build measurements before adoption. This issue is for user review and does not authorize implementation.
