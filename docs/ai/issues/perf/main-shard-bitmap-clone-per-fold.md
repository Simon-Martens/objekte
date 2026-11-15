# Main-shard fold copies fixed bitmaps

- Category: `perf`
- Originating task: `appendable-shards` (plan 3, C8; plan 3 D9)
- Branch: `integration/appendable-shards`
- Related measurement: `docs/ai/generated/appendable-shards-measurement.md`

## Observed behavior

Folding a small mutation shard into a collection's main shard clones the
main shard's fixed `RoaringBitmap`s: every posting index's `presence`, `nulls`
and `empty_selections`, plus `logical_records`, `empty_array_records`, the
inference bitmaps and the four relation bitmaps. The cost is proportional to
the main shard's record count, not to the appended delta.

In the 2,000-update Musenalm measurement, updates that folded had a median of
8.568 ms versus 3.450 ms for updates that did not fold (about 2.5x). The test
cycled over 2,000 files, so the main shard stayed at 1,912 records; a large
main shard would make the fixed-bitmap copy more visible.

## Expected behavior

A fold should cost work proportional to the small mutation shard, apart from
one documented term. Plan 3 explicitly keeps this term and defers its removal;
plan 5 slot reuse and vacuum reduce how often the main shard grows.

## Why it was not resolved in this task

Plan 3 D9 states that removing the remaining size-proportional term is out of
scope and must be measured and reported in this issue instead.

## Possible approaches (hypotheses, not implemented)

- Store the fixed bitmaps in a chunked copy-on-write container as well, so a
  fold copies only the touched chunks.
- Keep derived fixed bitmaps lazily and rebuild them from the posting families
  after a fold.
- Batch folds so several deltas amortize one fixed-bitmap copy.
