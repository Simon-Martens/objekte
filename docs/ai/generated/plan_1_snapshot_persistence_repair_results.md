# Plan 1 snapshot persistence repair: results

**Task:** `plan-1-snapshot-persistence-repair`  
**Base:** `c54afc3eb1cdc496734bfb30bcc9a3ea38988b78`  
**Attribution commit:** `bf39483f7c95368f222c760de04bde217b3dbe9d`  
**Repair commit:** `2cb74685e113e12dd6b5f34bc36f57781e4ea2d8`  
**Worktree:** `/home/simon/source/objekte-integration-incremental-index-availability`  
**Status:** representation repair implemented and measured on Plan 1; master merge review remains open.

## What changed

The base inventory is the authoritative metadata representation for each original file. The lifecycle catalogue and immutable segments now store 32-bit metadata references. An interner writes a supplemental metadata version only if the exact `FileEntry` differs from the base version or another supplemental version. Its equality includes ID, native root-relative path, revision, creation time, collection membership, status, diagnostics, and omitted-diagnostic count. This preserves historical moved/replaced/diagnostic versions in immutable segments. Retired catalogue slots, per-file fingerprints, file-version tokens, and the snapshot-wide configuration identity remain. A one-byte catalogue marker represents whether a file carries the validated common configuration identity.

Segment `file_index` and `id_lookup` are no longer serialized. The decoder builds each map once from the decoded file and record tables, rejecting duplicate file and record IDs before publication. Lookup keys clone the loaded `RecordId` handle and share its string allocation. `identity_order` remains serialized because it is the collection's sorted identity order, which cannot be reconstructed from file/record membership alone. Existing segment validation still checks slot, file, source revision, path-derived contribution, posting, sorting, inference, and relation domains.

An empty base `FileRecordIndex` now stores only `file_count` and empty offsets/slots. Valid in-domain file lookups return an empty slice, out-of-domain lookups return `None`, and generation validation rejects an empty offset table with records or slots. This compact form remains compact after restore. Snapshot versions changed from 23/24 to 27/28 so the earlier Plan 3 versions 25/26 can also be rejected after forward integration. Section checksums, bounded decode, trailing-input checks, atomic save, and prior-cache preservation remain in place.

## Exact encoded-byte attribution

Opt-in accounting reconciles each category to its completed snapshot length without a second full serialized snapshot. It decodes one already encoded segment at a time for attribution. These diagnostic runs are excluded from timing and RSS comparisons. Exact raw category maps are in [byte_attribution.json](../../../bench/results/incremental-index-availability/2026-11-13-snapshot-repair-pairs/byte_attribution.json).

| Encoded owner (bytes) | Before | Repaired |
| --- | ---: | ---: |
| Completed snapshot | 178,287,528 | 131,422,504 |
| Base inventory metadata | 16,602,104 | 16,602,104 |
| Lifecycle catalogue full metadata / references | 16,750,582 | 593,944 |
| Segment file full metadata / references | 16,602,160 | 594,008 |
| Supplemental metadata versions | absent | 8 (empty vector framing) |
| Catalogue fingerprint and configuration fields | 9,800,076 | 5,048,524 |
| Segment derived lookup/order payload | 6,385,090 | 594,008 (`identity_order` retained) |
| Empty base collection offset arrays | 4,157,692 | 56 |
| Segment index families | 88,626,137 | 88,626,137 |

The decrease is **46,865,024 bytes (26.29%)**. This fresh corpus has no semantically distinct historical metadata versions, so its supplemental table contains only framing. The interner and tests preserve distinct versions when mutations produce them. The repaired snapshot is **2,283,046 bytes above** the original master's 129,139,458-byte nonincremental snapshot; lifecycle liveness, file versions, per-file fingerprints, segment envelopes and reference arrays account for the remaining incremental state. This is a measured size difference, not an assertion that their runtime models are otherwise equivalent.

## Paired performance and memory

The principal campaign used seven alternating pairs of *fresh processes* for each of three workloads: construction alone, construction plus full save, and fresh restore. All 42 runs used the same Musenalm extended definitions, ASCII bigrams, 32 workers, rustc 1.99.1 release builds, `--profile`, a warm page cache, and the same 148,486-file corpus fingerprint. The before core was the reviewed Plan 1 code plus disabled-by-default diagnostic instrumentation (`bf39483f7c`); only the identical benchmark timing/restore harness was copied into its detached worktree. The after core was `2cb74685e1`. Query digest checks were identical in all 42 runs. Medians and inclusive Q1–Q3 follow; timings are ms, memory and sizes decimal MB. Full raw values, commands, manifests, hashes, and paired differences are in [runs.json](../../../bench/results/incremental-index-availability/2026-11-13-snapshot-repair-profile-pairs/runs.json) and [summary.json](../../../bench/results/incremental-index-availability/2026-11-13-snapshot-repair-profile-pairs/summary.json). A second unprofiled seven-pair campaign corroborates the direction of change.

| Workload and measure | Before median [Q1–Q3] | Repaired median [Q1–Q3] |
| --- | ---: | ---: |
| Construction: catalogue ready, ms | 227.94 [227.31–229.94] | 224.56 [218.75–225.16] |
| Construction: first searchable result, ms | 304.63 [302.03–309.18] | 298.88 [297.17–300.33] |
| Construction: full completion, ms | 921.43 [913.38–928.77] | 921.77 [909.52–925.94] |
| Construction: process peak RSS, MB | 566.16 [563.44–568.05] | 564.54 [564.05–564.72] |
| Construction: post-build RSS, MB | 557.14 [553.99–558.13] | 554.82 [553.74–555.47] |
| Build + save: **full external save call**, ms | 380.92 [379.57–384.89] | 293.98 [293.06–295.93] |
| Build + save: inner writer timer, ms | 198.05 [197.85–199.84] | 153.81 [153.72–154.27] |
| Build + save: process peak RSS, MB | 775.18 [772.04–779.27] | 715.42 [714.14–721.63] |
| Build + save: post-save RSS, MB | 602.51 [601.20–607.84] | 604.43 [602.35–610.29] |
| Fresh restore: complete load call, ms | 923.97 [914.49–938.52] | 733.36 [723.88–736.39] |
| Fresh restore: process peak RSS, MB | 492.11 [492.00–492.31] | 405.38 [405.23–405.46] |
| Fresh restore: post-load RSS, MB | 356.98 [356.84–357.06] | 328.26 [328.20–328.36] |

Construction source reads and parses were exactly 148,486 in every build-only run, before and after. Fresh restore reported **zero** source reads and build parses in every run. Restore query digests matched construction and save digests. The full save call includes lifecycle-state capture, segment encoding, writer work, sync, and publication; the inner writer timer starts later. The process high-water mark is cumulative within each workload and was never subtracted as if it were a phase-local allocation. A Unicode save/restore check produced 180,818,436 → 133,953,412 bytes with equal restore query digests and zero restore source reads; [unicode.json](../../../bench/results/incremental-index-availability/2026-11-13-snapshot-repair-pairs/unicode.json) contains the raw samples.

A fresh original-master nonincremental reference from the same machine/compiler produced 129,139,458 bytes, 534.37 MB construction peak and 541.26 MB build/save peak. Its [sample](../../../bench/results/incremental-index-availability/2026-11-13-snapshot-repair-pairs/master_reference.json) is a reference, not a paired comparison: master uses a different construction and snapshot path. The previously published seven-run master median save peak was 543.77 MB. We do not compare its query digest format directly with the incremental view's digest.

## Remaining save and restore buffers

The repaired lifecycle state holds all encoded segment `Vec<u8>` payloads while the outer snapshot streams them to a 64 KiB writer. The exact encoded segment payload total is **102,881,273 bytes** in the repaired diagnostic run. The separate profile campaign's repaired process peak is 715.42 MB versus 564.54 MB construction-only peak and 604.43 MB post-save RSS. These are *separate processes*, so their difference is evidence of a material save-time peak, not a measured allocation of 150.88 MB. The retained encoded buffers explain a large part of it; ownership, allocator behavior, and other temporary structures account for the remainder. Post-save RSS did not improve reliably despite the lower peak.

On restore, `read_section` verifies the checksum and deserializes the lifecycle section with those encoded segment buffers; segment indexes are then decoded while the encoded state remains alive. This overlap is consistent with the 405.38 MB restore peak and 328.26 MB post-load RSS, an observed 77.12 MB gap within the same process. The representation repair reduced that peak by 86.73 MB relative to the before median, but did not remove encoded/decoded coexistence. A bounded follow-up would stream segment encoding/decoding through separately checked records while preserving validation and atomic publication. [The performance issue](../issues/perf/lifecycle_snapshot_segment_buffer_peak.md) records this remaining cost for user review. This task did not change the format again to introduce that larger streaming rewrite.

## Validation and review state

- `cargo test --locked --workspace --quiet`: all packages passed; core library 523 passed, 16 intentionally ignored.
- Focused metadata reference, duplicate record ID, non-UTF-8 path, corrupt section, failed save, lifecycle mutation/restore, and old-format rejection tests passed. Legacy snapshot versions 23–26 are rejected by the new header.
- `cargo clippy --locked --workspace --all-targets`: passed with inherited warnings. A strict `-D warnings` invocation failed on 172 core test warnings across the existing workspace, so there is no clean strict-lint result.
- Save/restore correctness, five checked Musenalm query digests, and source-read counts passed across all paired trials. No readiness, first-result, or full-build regression exceeded the task's 10% plus 0.5 ms investigation threshold.

The Plan 1 repair is implemented and measured. The substantial remaining save peak is disclosed for merge review. Forward integration into Plans 2 and 3, their cumulative validation, and the separate desktop repair are tracked in their own completion records; **master remains unchanged**.
