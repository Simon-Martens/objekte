# Root-relative paths and shared identity storage: results

Task: relative-path-and-identity-storage. Baseline: 64a09886c5fed963fb0f53df779c6ad6c1a008f1 (the actual pre-task origin/master on 12 November 2026). Integration branch: integration/relative-path-and-identity-storage. Implementation commits: 7e71100040 (paths) and 72ba7cfd07 (record IDs). Master was not changed.

## Design and compatibility

The file table, exact lookup, and ordered path index now use root-relative paths. Ordinary native paths and query paths share one immutable Arc string allocation across the file entry and both lookup keys. A path requiring reversible escaping or containing invalid UTF-8 uses a native Arc path and a separate escaped query key. The single repository root remains in configuration; operating-system paths are joined at source access boundaries. Published lifecycle views keep their own immutable path objects on move.

SearchHit.file_path, @file.path, list/search/streaming projections, CLI output, TUI rows, and desktop projections use the root-relative value. The result field names are unchanged. Absolute-looking filter literals are compared literally and are not rebased. Source operation inputs continue to accept contained absolute paths. The previous base ordered path index held absolute keys, so an exact absolute @file.path query could select a key but then fail its relative-value check. The new path query returns the matching file with an index-proven result and no source scan.

RecordId now wraps Arc<str>. Fresh build, base snapshot restore, lifecycle segment decode, compaction, and lookup cloning share each resident record's ID payload with its identity lookup key. Borrowed string lookups avoid a temporary ID allocation. ID equality, lexical ordering, and external string serialization are unchanged. The path/index snapshot versions are now 23/24: required-load rejects older derived path semantics, while IfValid rebuilds. Tests cover both policies and restored payload sharing.

## Validation

| Check | Result |
| --- | --- |
| cargo check --workspace --locked | Passed |
| cargo test --workspace --locked | Passed; 511 core tests, 78 CLI tests, 33 and 168 desktop test groups, and other workspace targets; no failures |
| cargo clippy --workspace --all-targets --locked | Passed with existing repository warnings |
| cargo test --locked -p objekte-desktop --example environment_validation | Passed (one native control test) |
| Extended Musenalm query suite | 194 cases in ASCII and Unicode, 388 case/profile rows; zero failures and identical ordered result digests, counts, returned rows, and source-work counters |

The focused tests also verify two same-named files in different directories, nested collections, forced scan/index parity, snapshot restore and required-load rejection, lifecycle add/update/move/delete with retained views, percent and backslash names, invalid UTF-8 paths on Unix, CLI execution from another working directory, and pointer sharing for fresh, base-restored, and lifecycle-restored IDs. Existing relation and duplicate-ID tests passed, as did the new "001" versus "1" string identity test.

A stricter Clippy invocation with -D warnings was attempted and stopped on existing unrelated warnings in bigram/build and other modules; the documented non-deny invocation above passed. No warning was suppressed for this task.

## Matched build and memory measurements

The clean control binary was built from 64a09886c5; the clean candidate binary from 72ba7cfd07. Both used Cargo's release profile, rustc 1.99.1, four build workers, profiling disabled, the same Musenalm corpus, and the same machine (AMD Ryzen 7 5700G, Linux 7.2.13, powersave governor, CPUs 0-15). There was one warm-up per role, then three alternating fresh-process control/candidate pairs per alphabet. Corpus fingerprint: db869a76f26e1a3275a5162e5a207f47fb50fa8be02c4bdcfe3f1db50cfb747c. Build-harness schema fingerprint: 4115070eb30b0af98b109429d0883b493f857cdf671ec55b943d91b73c5ebfe3. The query manifest bytes had SHA-256 edd9ace84c1df94692e4c626d942d334b4fae0731f258477267be97d3ed967b2 in both checkouts.

| Alphabet | Build median, control → candidate | Paired change | Post-build RSS median | Peak RSS median | Snapshot bytes, both |
| --- | ---: | ---: | ---: | ---: | ---: |
| ASCII | 2,535.060 → 2,498.069 ms | −1.46% | 441.86 → 406.86 MiB | 460.72 → 426.02 MiB | 129,139,458 |
| Unicode | 2,944.655 → 2,854.103 ms | −3.08% | 482.44 → 424.78 MiB | 501.22 → 446.25 MiB | 131,650,034 |

Snapshot sizes came from the same control/candidate query-harness builds on a disposable corpus copy. They are identical because both path and ID payloads serialize as the same strings; the savings are resident ownership and derived-index storage. RSS changes are process measurements and need not match estimated live-byte savings exactly.

The opt-in --storage-diagnostic runs after the timed build. It counts shared payloads once and reports table lengths/capacities, entry sizes, native path payloads, lookup slots, escaped query keys, ID payloads and lookup references. The Musenalm paths needed no separate escaped keys. Baseline diagnostic code was applied only after the clean timing runs; its patch is retained with the raw evidence and the control worktree was restored clean.

| Affected component (estimated bytes) | Control | Candidate | Change |
| --- | ---: | ---: | ---: |
| File table, paths, lookup slots, root and membership vectors | 83,675,196 | 60,836,688 | −22,838,508 |
| Record table, unique IDs and identity lookup | 39,901,556 | 33,467,786 | −6,433,770 |
| Total changed storage | 123,576,752 | 94,304,474 | −29,272,278 |

Both file tables have 148,486 entries, capacity 262,144, and 160-byte entries. The exact lookup capacity is 229,376 in both. The baseline stores 5,020,188 bytes of separate exact-lookup path payload and 19,006,208 bytes of absolute ordered-key payload; the candidate stores neither. Ordered key slots fall by 1,187,888 bytes. Candidate native-path payload accounting rises by 2,375,776 bytes because the estimate includes Arc allocation headers; those payloads are shared by the other indexes. The record table entry size falls from 96 to 80 bytes, saving 4,558,976 bytes at its measured capacity. Identity lookup slots save 2,023,280 bytes, and 2,227,290 bytes of duplicate lookup-ID payload disappear. Candidate unique-ID payload accounting includes Arc headers and rises by 2,375,776 bytes. These figures explain the net reduction without double counting shared data.

The historical 83.68 MB FileInventory estimate covers the file table, paths, path indexes, root and file membership vectors. It excludes record tables and record-ID lookups. This report preserves that historical raw number and separately accounts for the IDs. The diagnostic omits hash-table control bytes, B-tree node allocations, allocator rounding, diagnostics, and unrelated indexes; serialized bitmap sizes are not used as a total heap estimate.

## Focused operations and query timing

Seven-block release probes per fresh process used 100,000 iterations for exact file and ID lookup, 500 for source-byte materialization, and 100 for each candidate path query. Three alternating measured pairs followed warm-up. Median call costs were 44 → 43 ns for exact native path lookup, 16 → 16 ns for borrowed ID lookup, and 4,932 → 5,034 ns for source materialization. Candidate root-relative @file.path equality and bounded range queries returned the same one file, with one index-proven candidate and zero source reads; their median call costs were about 0.67 ms and 0.061 ms. Baseline absolute equality took about 0.63 ms but returned zero, so it is not a valid answer-equivalent timing comparison. The equality plan spent about 0.7 ms in planning in both versions; the new path index did not introduce a scan.

The existing lifecycle small-fixture benchmark exercised multi-record file add, append update, verified update, move and verified delete on independent disposable roots. In 20 additional alternating pairs, control → candidate medians were 0.2505 → 0.2590 ms (add), 0.1505 → 0.1480 ms (append), 0.1695 → 0.1660 ms (verified update), 0.0745 → 0.0740 ms (move), and 0.0340 → 0.0335 ms (delete). The add difference is 8.5 microseconds on a tiny fixture; no operation showed a material regression.

The 194-case suite's one-repetition timing flagged five rows above 5% and 1 ms. A 21-repetition recheck removed the apparent Q158, Q164, and Q190 regressions. Alternating follow-up blocks put Q177 at +1.5% and Q119 at +4.7% (+1.2 ms). Q119 is a broad @file.modified count; baseline and candidate both prove 133,948 matches through the index, open no source files, and spend about 7.23 ms in planning. The remaining small wall-time difference is outside path/ID lookup work; its precise cause is not established. The user accepted the benchmark result and directed us to finish rather than pursue this small timing difference. In 21-repetition focused runs, Q034 and Q047 ID predicates changed by +3.3% (0.19 microseconds) and +0.8% (0.51 microseconds), while Q149's relative path text predicate improved by 3.0%; all ordered digests match.

## Raw evidence and limits

The compact raw measurements are under [bench/results/relative-path-and-identity-storage/2026-11-12](../../../bench/results/relative-path-and-identity-storage/2026-11-12/). Build pair manifests and samples are in build-measurements/ascii and build-measurements/unicode. Query manifests, 388 result rows, samples, and digests are in queries/control and queries/candidate. Component samples and the temporary baseline instrumentation patch are in storage-diagnostic. Operation and path-plan samples are in operation-probes. The 20-pair mutation samples and focused query rechecks are in the corresponding files/directories. Large snapshot binaries and the disposable corpus copy stayed in ignored target/relative-path-storage; their byte sizes and fingerprints are preserved in the committed JSON evidence.

The changed benchmark candidate was 72ba7cfd07. The final reporting commit adds opt-in diagnostics and operation probes plus this report; it does not alter the measured default build path beyond a mechanically equivalent SharedPath conditional cleanup. The estimates describe the measured base generation, not every possible lifecycle overlay. Escaped native paths need a separate reversible query key, whose live bytes are reported independently when present.
