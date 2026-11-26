# Objekte repair — continue a file-backed result page after an unavailable source

**Reviewed head** `integration/stream-memory-posting-comparison` at `b16f8401f60b40b0fa296983fd71a70e2cc8c11e`. The last implementation commit is `f573242ab2`. The retained-payload budget and named String/ID comparison regressions are covered. This repair concerns finite ordered payload streams on a directory that certifies disjoint ascending record-ID ranges and disjoint file associations. The default memory directory leaves this optimization disabled. [Directory contract](https://github.com/Simon-Martens/objekte/blob/b16f8401f6/crates/core/src/index/storage/segment_catalog.rs#L64-L74)

## Failure and expected result

Index records A, B and C, each in its own physical file. Give them the same indexed numeric value. Pin the view, then rename only A’s file so its posting and locator still exist but its payload is unavailable. Request the first matching row with bytes, no count, no evidence and no explicit sort. Use this same logical dataset in each layout.

| Certified placement layout | Result implied by the current loop | Expected record |
| --- | --- | --- |
| A and B together, then C | C | B |
| A alone, then B and C together | B | B |
| A, B and C together | Empty page | B |

The outer loop asks each placement once for at most the remaining number of **locators**. When payload lookup fails for A, it records the diagnostic and advances to the next placement without asking for B in the current one. The ordinary file-backed ordered stream continues its cursor after an unavailable payload and stops when it has filled the requested page. These results were traced in source and checked in a small control-flow model. The review environment had no Rust toolchain. [Locator page selection](https://github.com/Simon-Martens/objekte/blob/b16f8401f6/crates/core/src/query_execution/runner/mod.rs#L5511-L5549) · [Placement advancement](https://github.com/Simon-Martens/objekte/blob/b16f8401f6/crates/core/src/query_execution/runner/mod.rs#L5577-L5606) · [Ordinary ordered stream](https://github.com/Simon-Martens/objekte/blob/b16f8401f6/crates/core/src/query_execution/runner/mod.rs#L5197-L5231)

## Implementation and acceptance

1. Add the three-layout regression to the existing certified-directory transfer tests. Assert each layout’s actual record IDs per placement before renaming the file. Verify that a locator request still returns A. Use the established healthy-source control and disable the verified-result cache.
2. For file payloads, resume at the next candidate **in the current placement** after an unavailable row. Advance to the next placement only once the current one is exhausted or stop when the requested page is full. Prefer the existing ordered-slot cursor where available. Keep early stopping on healthy sources and keep buffering bounded. Preserve the existing offset semantics, cancellation, and typed failure handling. Count each candidate and diagnostic once.
3. Assert that all three layouts return B for both byte and logical payload delivery. Check A’s single diagnostic, an incomplete overall match count, and no access to C’s placement once B fills the page in the first layout. Keep the 96 KiB retained-payload and one-placement locator regressions green.
4. While changing this delivery path, cover its file I/O counters. On an exact posting hit from one healthy nonempty file, assert the selected bytes, primary bytes read and one distinct physical file. At the reviewed head, this path increments selected payload bytes and file opens while omitting the corresponding primary bytes and distinct file count. Reuse the ordinary file-read accounting and avoid charging the same file twice for distinct-file statistics. [Certified accounting](https://github.com/Simon-Martens/objekte/blob/b16f8401f6/crates/core/src/query_execution/runner/mod.rs#L5698-L5717) · [Existing stream accounting](https://github.com/Simon-Martens/objekte/blob/b16f8401f6/crates/core/src/query_execution/runner/mod.rs#L5224-L5232)

Run the focused regressions and the locked core suite. Report the actual record IDs for each placement layout and the commit tested. The previous implementation report records 714 passing core tests, 16 ignored, 12 passing release transfer tests, and 150 matching Musenalm query digests. [Previous verification](https://github.com/Simon-Martens/objekte/blob/b16f8401f6/docs/ai/done/objekte_stream_memory_and_posting_comparison_repairs.md#L39-L74)

## Implementation report

Completed 2026-11-26 in `integration/file-backed-stream-page-continuation`.
The reviewed base is `b16f8401f60b40b0fa296983fd71a70e2cc8c11e`.
The implementation and tested commit is `ab4bfc76265c7a84ec903ef31f8b5f8dce0c76d9`.
The integration worktree is `/home/simon/source/objekte-integration-file-backed-stream-page-continuation`.
All changes were made in that worktree.

Certified file-backed payload streams now use the ordered traversal within
each placement. An unavailable payload records one diagnostic and leaves
room for the next candidate. The traversal uses stored identity order when
available and sorts the current placement's candidate identities under
the stream memory budget otherwise. It emits one payload at a time and
carries progress across placements. Offset handling and typed cancellation,
resource, and source revision outcomes have regression coverage.

The test verified each placement's record IDs before renaming A's file.
A, B, and C had separate physical files and the same indexed score.

| Actual placement record IDs | Healthy bytes | Locator after rename | Bytes after rename | Logical after rename | Payload placements visited after rename |
| --- | --- | --- | --- | --- | --- |
| `[A, B] ⎮ [C]` | A | A | B | B | 1 |
| `[A] ⎮ [B, C]` | A | A | B | B | 2 |
| `[A, B, C]` | A | A | B | B | 1 |

Each payload request after the rename visited two candidates, recorded
one diagnostic for A, and reported the match count as incomplete. In the
first layout an injected directory refill failure verified early stopping
before C's placement. The third layout used a backend without a stored
identity order. The healthy exact posting hit read 20 selected payload
bytes and 20 primary source bytes from one distinct file. The retained
payload regression emitted all four rows under its 96 KiB budget with a
50,633-byte observed peak. The locator early-stop regression read one
placement.

```sh
cargo fmt --all -- --check
git diff --check
cargo test --locked -p objekte-search-core --lib transfer -- --nocapture
cargo test --locked -p objekte-search-core --lib
cargo test --locked --workspace
cargo test --locked --release -p objekte-search-core --lib transfer -- --nocapture
```

Formatting and whitespace checks passed. The locked core suite passed
715 tests with 16 ignored. The locked workspace suite passed 1,122 tests
with 22 ignored across its test binaries and doctests. All 13 focused
transfer tests passed in debug and release. Validation used rustc 1.100.0
on Linux with a 32-thread Intel Core i9-13900K.

```sh
cargo build --locked --release -p objekte-cli --example bench_musenalm
bench_musenalm --root /home/simon/source/objekte/examples/musenalm --profiles ascii-off --repetitions 1 --result-dir <output>
```

The Musenalm replay used 32 query workers and the `ascii-off` profile
with verified caching disabled. The reviewed release binary was built from
`f573242ab293f24227675ace48ef2282ae4ab130`, whose only difference from the
reviewed head is the previous completion document. Both runs used the same
dataset and byte-identical query lists. All 150 result digests and all
compared query work counters matched. The SHA-256 of the emitted
`result-digests.json` is
`2921db4219e1045db945049a45e56c0aa71cc14e601c3a33dc24269d6e64ff29`.
The median per-query candidate-to-reviewed latency ratio was 0.996 in
this single replay. Musenalm uses the default directory, so the release
transfer tests provide direct coverage for the certified path.

The integration worktree is retained for review. The original untracked
specification and the task's temporary notes are removed after verifying
that the report is present on the remote and preserves the specification.
