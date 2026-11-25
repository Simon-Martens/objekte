# Bound empty-chunk staging for registered JSON Lines readers

Start from `integration/ingestion-followup-merge-review` at `5a637172214b13f1982d130aafc0ad65b4ce8455`. The implementation under review is `d2ae68aeb7e1d24673b4ce0f622c8fa417b8101d`. This correction completes item 2 of the ingestion merge review.

## Reproduce through the public build

1. In the existing lifecycle initial-build test module, register `Arc::new(JsonLinesReader)` under a fresh source ID and file extension. Select that registration for the collection. This gives the reader extension provenance and selects `supports_bounded_json_lines_build()`. The existing header-reader regression selects `supports_streaming_build()`, which exercises a different function.
2. Create two physical files for the collection. Give the peer one valid record. For the main file, write 16 lines, each exactly 1 MiB including the terminating newline. Fill each line with ASCII spaces. Repeat with 64 lines. Reuse a single buffer to write the fixture. Set the segment record target above one and the maximum files per segment to one.
3. For each length, exercise both an all-blank main file and a main file with one valid JSON record appended after the final blank line. Use `Index::start_build` and the existing streaming budget high-water counters.
4. At the reviewed revision, each full blank line produces one byte-filled chunk with zero ranges. The bounded-line path schedules it, builds a file contribution and passes it to `stage_stream_binding`. The tail grows by one object while the segment record count stays at zero. Record the observed unsealed contribution peak before the fix.

[Framing](https://github.com/Simon-Martens/objekte/blob/5a637172214b13f1982d130aafc0ad65b4ce8455/crates/core/src/source/framing.rs#L163) · [Reader dispatch](https://github.com/Simon-Martens/objekte/blob/5a637172214b13f1982d130aafc0ad65b4ce8455/crates/core/src/construction/file_preparation.rs#L1915) · [Bounded-line scheduling](https://github.com/Simon-Martens/objekte/blob/5a637172214b13f1982d130aafc0ad65b4ce8455/crates/core/src/construction/file_preparation.rs#L2292) · [Group staging](https://github.com/Simon-Martens/objekte/blob/5a637172214b13f1982d130aafc0ad65b4ce8455/crates/core/src/construction/initial.rs#L120)

## Implementation

In the bounded JSON Lines preparation path, consume and account for a zero-range frame as framing work. Release its input capacity before taking the next frame. Schedule decoder work for frames that contain records. When EOF is reached after a successful file with no records, supply file membership exactly once. Preserve the first-record-bearing batch's file accounting when leading frames were empty. Apply the same behavior to blank spans in the middle of and after records.

Keep the source digest and revision checks over the complete input, absolute record locators, diagnostic line origins, input limits, and framing statistics. Check cancellation between skipped chunks so an all-blank file can stop cooperatively. Propagate framing or source errors and drain already dispatched work through the existing failure path. Keep segment packing and the current decoder scheduler.

## Acceptance

- With the two-file fixture, the unsealed contribution peak stays at most two for both 16 and 64 blank frames, with and without the final record. Record actual peaks. Increasing the blank prefix adds framing work without adding decoder batches for those frames.
- A successful blank file appears once in the catalog and has zero records. With a final record, the catalog returns one record for that file. Its byte range starts at the blank prefix length and ends after the JSON bytes, excluding the line terminator. The peer remains queryable. Include either a focused diagnostic assertion or an existing test that covers line numbers after blank lines.
- Preserve late-error atomicity. Exercise a late read or framing failure on the bounded path using an existing reader fixture if available. Preserve the existing 30,000-frame callback regression, registered-group cancellation and draining tests, published query domain tests, and the mixed-file overlap test.
- Run the focused tests and the locked workspace suite. Report the pushed SHA and before/after high-water counts. The existing paired construction measurements remain the baseline; this correction has a direct memory-growth acceptance test.


## Implementation report

Completed 2026-11-25 on `integration/bounded-jsonl-empty-chunk-staging` in `/home/simon/source/objekte-integration-bounded-jsonl-empty-chunk-staging`. The reviewed parent is `5a637172214b13f1982d130aafc0ad65b4ce8455`. The tested implementation is `6a764ac4def1307bc06a41d6cb9c826bc9e356d9`.

The bounded JSON Lines scheduler accounts for every frame and drops zero-range input before taking the next frame. It schedules only frames with records. Successful files without records contribute one file membership at EOF with zero decoder jobs. The first scheduled frame carries the file's first-range marker. The same loop checks cancellation before continuing past each skipped frame and drains dispatched work on a late framing error.

### Public reproduction

The fixture registers `JsonLinesReader` under a custom source ID and extension, uses two physical files and configures one file per segment. The main file has 16 or 64 blank 1 MiB lines. The peer contains one record in every run. Framing totals include the peer. The reviewed implementation fails the high-water assertion.

| Blank frames | Final record | Reviewed unsealed peak | Repaired unsealed peak | Frames processed | Bytes framed |
| ---: | --- | ---: | ---: | ---: | ---: |
| 16 | No | 16 | 1 | 17 | 16,777,230 |
| 16 | Yes | 17 | 1 | 18 | 16,777,244 |
| 64 | No | 64 | 1 | 65 | 67,108,878 |
| 64 | Yes | 65 | 1 | 66 | 67,108,892 |

Every successful main file has one collection membership. Reverse lookup returns zero records for an all-blank file and one for a file with a tail. Tail locators start at 16,777,216 or 67,108,864 and span the 13 JSON bytes. The full-input SHA-256 and peer lookup also pass.

### Validation

- `cargo test --locked -p objekte-search-core --features detailed-profiling --lib bounded_jsonl -- --nocapture` passed five tests. The bounded reader fixture covers a failure before the first chunk, an injected late read failure while decoder work is outstanding, absolute diagnostic line 4 after interleaved blanks, and cancellation during an all-blank file. The all-blank case frames two chunks, stages one membership and records zero active decoder jobs.
- `cargo fmt --all -- --check`, `cargo check --locked --workspace --all-targets`, `cargo test --locked --workspace` and `cargo run --locked -p objekte-search-core --example record_ingestion_public` passed. The suite includes 732 passing core unit tests and 26 passing record-ingestion integration tests. The existing 30,000-frame callback, registered cancellation and draining, published query domain, and mixed-file overlap regressions all pass.
- This correction uses the direct memory-growth fixture for measurement. The earlier paired release measurements remain the construction baseline. [Paired construction report](objekte_ingestion_followup_merge_review.md)

The integration worktree is retained for review. The supplied task text is preserved byte for byte above.
