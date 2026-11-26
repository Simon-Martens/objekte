# Objekte storage backend review — stream memory and posting comparison

**Decision.** Hold `integration/storage-backend-repair-review` at `0913cc5ee39ed68184515d25fe2f5c418edbc098` for the two corrections below. The last code commit is `c5dcec2b28`. Continue from this head and follow the repository's integration-branch workflow. The earlier empty-array fix, directory scoping, bounded gram extraction, numeric range access and metadata sort updates already have targeted regressions.

## 1. Enforce the caller’s memory budget when streaming a certified directory

**Observable problem.** `try_stream_identity_directory_page` uses buffered search to obtain an entire placement’s page, then splits the returned hits into delivery events. The exact-posting path charges the winner heap for locators. Later, buffered search materializes the selected logical payloads and holds them all in a vector. The directory shortcut checks the size of each event but never accounts for that live payload vector in `Delivery::check_buffer`. A sibling file-backed optimization also constructs fresh default stream options when invoked through buffered search. A caller can receive a successful completion above its specified memory limit and a zero peak for a nonempty stream. [Directory shortcut](https://github.com/Simon-Martens/objekte/blob/0913cc5ee3/crates/core/src/query_execution/runner/mod.rs#L5509-L5557) · [Locator construction](https://github.com/Simon-Martens/objekte/blob/0913cc5ee3/crates/core/src/query_execution/runner/mod.rs#L2477-L2503) · [Payload retention](https://github.com/Simon-Martens/objekte/blob/0913cc5ee3/crates/core/src/query_execution/runner/mod.rs#L3777-L3863) · [Default budget in buffered ordered search](https://github.com/Simon-Martens/objekte/blob/0913cc5ee3/crates/core/src/query_execution/runner/mod.rs#L2326-L2355)

**Regression fixture.** Extend the existing certified-directory fixture. Submit four direct retained records **in the same batch and placement**. Give each a short unique ID, an indexed integer `score = 7`, and a 48 KiB string payload. Confirm the placement count. Query the score posting, requesting logical rows, no evidence, no explicit sort, no count, offset zero and limit four. Set `batch_rows = 1`, `max_event_bytes = 64 KiB` and `max_buffered_bytes = 96 KiB`. Each row fits an event, while the four strings alone need 192 KiB. The existing one-row-per-placement locator fixture cannot expose the error. [Existing fixture](https://github.com/Simon-Martens/objekte/blob/0913cc5ee3/crates/core/src/index/storage/segment_catalog/tests/segments/transfer.rs#L1291-L1370)

**Implementation.** Prefer reusing the budget-aware streaming and payload delivery code. A small first fix may route payload-bearing requests to the established streaming path and keep early directory stopping for locator requests. For the retained shortcut, carry the caller’s actual options through any nested ordered search and account for live result storage, including locator rows and cursor state where applicable. Bound allocations and report `peak_buffered_bytes` through the outer delivery. Summing the completed vector after materialization alone leaves peak allocation unbounded. Preserve cancellation, event order, offsets, and the existing one-placement early stop for a limit of one.

**Acceptance.** The four-row case either emits identical rows in order within the budget, with an observed nonzero peak, or returns the existing typed resource error at the established budget boundary. Verify the high-budget control returns all four rows. Check peak accounting on a successful locator request as well. Keep the existing injected directory-refill error tests.

## 2. Align placed string and ID posting restrictions with filter comparison

**Observable problem.** The reference comparison treats `Value::String("A")` and `Value::Id("A")` as equal. The default matcher for a registered operation calls this comparison after mapping each record value. The posting reader contract limits a restriction to keys of the anchor’s scalar type. The placed planner tests the type of the **declared field** and the prepared literal, then can advertise an exact result without proving that the operation’s mapped keys have the same representation as that literal. A conforming store can omit a required record. The current independent test store selects only unnamed numeric fields. [Reference comparison](https://github.com/Simon-Martens/objekte/blob/0913cc5ee3/crates/core/src/query/semantics/comparison.rs#L100-L109) · [Default binding matcher](https://github.com/Simon-Martens/objekte/blob/0913cc5ee3/crates/core/src/query/binding/matcher.rs#L9-L25) · [Posting reader contract](https://github.com/Simon-Martens/objekte/blob/0913cc5ee3/crates/core/src/index/storage/posting.rs#L275-L317) · [Proof eligibility](https://github.com/Simon-Martens/objekte/blob/0913cc5ee3/crates/core/src/index/memory/access/segment.rs#L146-L229)

**Regression fixture.** Declare an indexed scalar string field `code`. Register a named, complete, posting-enabled binding that maps each string to `Value::Id` with the same characters and prepares a quoted literal as `Value::String`. Use the existing default mapped matcher. Extend the independent posting fixture for this named field and preserve the actual emitted key variants. Insert records A and B with codes `"A"` and `"B"`. Select the named binding explicitly. Equality with `"A"` must return only A, and inequality with `"A"` must return only B. Compare IDs and exact counts to the reference evaluator with evidence and caching off. Assert that independent construction was selected. This example uses a declared String field throughout ingestion, so schema coercion of a declared ID field is irrelevant.

**Implementation.** Make exact-proof eligibility reflect the keys the operation actually produces. A conservative solution can decline the exact operation when mapped text-key compatibility is unknown and retain the entire affected input for verification. An indexed solution can probe both text key variants for equality and implement inequality and ranges using the reference comparator across both variants. Leave ordinary numeric restrictions and their selective-input path intact. Run the same semantic fixture with native placement and correct any shared proof error it exposes. A candidate result that already excludes matching IDs cannot be repaired by residual verification.

## Completion

Run the focused storage, streaming and named-binding regressions and the locked core suite. Include the four-row outcome, the observed peak or typed error, the exact IDs for both named-binding queries, and the reviewed commit SHA in the integration report. The previous run recorded 712 passing core tests with 16 ignored in both workspace configurations, 11 passing release transfer tests, and 150 matching Musenalm query digests. These two findings come from tracing the source. The review environment had no Rust toolchain for executing the new regressions. [Previous implementation report](https://github.com/Simon-Martens/objekte/blob/0913cc5ee3/docs/ai/done/objekte_storage_backend_repair_review.md#L62-L127)

## Implementation report

Completed 2026-11-26 on `integration/stream-memory-posting-comparison`.
The reviewed base is `0913cc5ee39ed68184515d25fe2f5c418edbc098`, and the
integration worktree is
`/home/simon/source/objekte-integration-stream-memory-posting-comparison`.

| Commit | Change |
| --- | --- |
| `ba77fe7909` | Native and placed posting proofs require matching mapped textual key types. The fixture verifies that the independent writer receives ID keys while the prepared literal remains a String. |
| `b5af00dc99` | Certified finite streams select budgeted locator rows and materialize one payload at a time. The outer delivery reports the ranked locator and payload retention estimates under the caller's options. The nested locator evaluation bypasses the default-budget buffered ordered shortcut. |
| `f573242a` | Native named posting construction records the mapped value types so compatible operations can keep exact index proofs. |

The retained regression uses four rows from one placement, each with a
48 KiB logical string. The 96 KiB request and the 512 KiB control each emit
`row-0`, `row-1`, `row-2`, and `row-3` in order. Both report a
**50,633-byte** peak. A tighter 48 KiB request returns the typed resource
error before any row is emitted. The existing one-row locator request
reports a nonzero peak and reads one placement. A complete-count request
still reaches the injected refill failure.

Both native and independent placement return exactly `path:a.row`
for `code:as_id = 'A'` and exactly `path:b.row` for
`code:as_id != 'A'`. Each count is one and agrees with the reference
evaluator. Mapped text with unproven or mismatched key types uses
residual verification. The existing compatible named bindings and
selective numeric posting tests retain their indexed paths.

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked -p objekte-search-core --lib index::storage::segment_catalog::tests -- --nocapture
cargo test --locked -p objekte-search-core --lib query_execution::restrictions::tests
cargo test --locked -p objekte-search-core --lib
cargo test --locked --workspace
cargo test --locked --workspace --features objekte-search-core/test-scan-instrumentation
cargo test --locked --release -p objekte-search-core --lib transfer -- --nocapture
```

Formatting and the locked all-targets check passed. The storage fixture
group passed 27 tests and the query restriction group passed 12. The
locked core run and both workspace configurations each passed 714 core
tests with 16 ignored. All 12 release transfer tests passed.

The Musenalm replay used the same repository dataset and `ascii-off` profile
as the reviewed revision. All 150 result digests matched. The canonical
digest map SHA-256 is
`53fbe3252acb0a8e9d4715adc7d695ec8623c1eb5c9319f274960ef482e4fc28`.

```sh
cargo build --locked --release -p objekte-cli --example bench_musenalm
bench_musenalm --root examples/musenalm --profiles ascii-off --repetitions 1 --result-dir <output>
```

The stream measurements cover core-owned result buffers. File and provider
readers still use their existing per-record limits and reader APIs.
The integration worktree is retained for review. The temporary notes and
original untracked specification are removed after verifying the report push
and the preserved specification bytes.
