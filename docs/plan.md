# Core allocation and copying work plan

Starting point: `master` at `ccf61c814d8d70d308d46196eeae447694ccc844`.
Work in the existing architecture. Preserve public API, sorting and comparison
semantics, freshness checks, bounded parallelism, index and snapshot formats.

## Baseline

- Inspect the four paths and run the existing core tests before changing code.
- Measure allocation count, allocated bytes, peak live heap, peak RSS, and wall
  time in release builds with repeated samples. Use Musenalm and a larger corpus.
- Separate warm queries from cold construction and snapshot saving. Benchmark
  indexed 25-row views; unindexed filtered sorted 25-row views with and without
  count; broad sorted JSON and text 25-row queries; cold sort-index rebuild;
  secondary snapshot save and restore.
- Use existing sort and query-mix benchmarks where useful, and profile with
  Heaptrack or an equivalent counting allocator. Record any change in priority
  supported by measurements before making it.

Baseline finding: the selected view fallback is material (68.4 MB peak live
growth for 20,000 records and a 25-row page); broad Musenalm sorted JSON
queries allocate 11.6 GB cumulatively but have only 11.1 MB peak live growth.
The latter largely reflects parsing and verification across 133,161 source
reads, so it does not displace the four requested redundant-copy changes.

## Change 1: Bounded view result retention

In `repository/views.rs`, `query.rs`, and related sorting code, retain at most
`offset + limit` matching candidates with safe arithmetic and O(log K) selection
for large K. Calculate exact count without retaining all matches. Avoid building
unreturned `ViewRow`s, validating unreturned documents unless the direct-mode
contract requires it, and retaining full JSON for selected pages when keys and
projected rows suffice. Preserve null, missing, invalid, numeric, descending,
and ID tie-break semantics, states, and source freshness. Avoid quadratic
behavior for large offsets.

## Change 2: Single parse for sort-index projection

In `file_picker/projection.rs`, `file_picker/sort_index.rs`, `sorting.rs`, and
`file_picker/rebuild.rs`, use the existing Sonic parse for sort keys instead of
parsing full JSON again with serde_json. Support nested paths, array reductions,
ICU language, exact numeric lexemes, duplicate-key last-member-wins, malformed
input, and recursion limits. Investigate worker-local source buffers without
retaining borrowed bytes. Preserve text, typed, presence, view, and sort indexes.

## Change 3: Optional sorted-query progress snapshots

In `repository/json_query/sorted.rs`, `repository/text_query/sorted.rs`, and
their callers, make the internal progress callback optional. Normal sorted
queries must not deep-clone intermediate pages. Explicit stream-update APIs
keep their signatures and progression. Preserve final rows, totals, truncation,
source reads, parses, and single-pass parsed-document reuse.

## Change 4: Borrowed view-index serialization

In `file_picker/view.rs` and `file_picker/secondary.rs`, serialize borrowed
view-field, scalar-key, and ordering slices rather than deep-cloning them.
Retain owned restoration. Keep binary compatibility, checksums, validation,
atomic replacement, and corruption rejection. Report larger unrelated
snapshot allocations separately.

## Verify and deliver

After each task run relevant tests, complete core tests, `cargo fmt --all --
--check`, `cargo clippy -p objekte-core --all-targets -- -D warnings`, and the
same benchmark cases. Record wall time, allocation count and bytes, peak live
heap, peak RSS, and independent result parity. Test small records and large
nested JSON; use ignored real Musenalm tests where useful. Make four reviewable
commits, preferably one per task. Report mechanism, correctness, complexity,
benchmark results, tests, remaining limits, and largest remaining allocation
sources. Remove this plan file when all four changes are complete.
