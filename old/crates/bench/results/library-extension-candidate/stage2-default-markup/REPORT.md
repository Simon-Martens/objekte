# Default markup representation route, 2026-11-04

## Revisions and inputs

- Reference production source: `13068fbd010e7034435e4272d659418d1c8e173e`, pinned before changing production code. Candidate implementation: `b84495f815` on `library-extensions-stage2-wip`.
- Same `examples/musenalm` paths and metadata in both processes; 148,486 repository records, including 133,948 `contents` build sources. Source content and stat fingerprint: `90e36ad63dcb6386e9521c583fdcd36451fd3d6182187dced542fb8292f730ce`. Configuration checksums are in `manifest.json` and `paired-final/manifest.json`. Lockfile SHA-256: `2e219967e2327f963a8a7863bc649fdcccaf52b555de336604a810d72f003e52`.
- Rust/Cargo 1.99.1, `x86_64-unknown-linux-gnu`, workspace release flags, `RAYON_NUM_THREADS=16`, affinity CPUs 0–15. Filesystem caches were warm and were not evicted. Reference source was built in a sparse detached worktree, with all benchmark binaries pointed at the original repository data paths.
- Preserved ignored executables: FTS reference SHA-256 `f4b4433e12941e04b2a4d7bf489607e4e375c0a37121839461d6ad10dad8b917`, final candidate `5ad2c79ad8ac386efcd383b93d8a5dcd37ec333417becd9dcd65c877350b2ea2`; index-build production reference `1a23d9d96c8a3485b114352ea61a20d2cf3d52e1c466092628beae43afaeea53`, final candidate `80cbf6d6d514d5092fe94a270405b9053f0c7bc96dfb157aa9fb7b316633d486`; allocation-counted reference `51b45fdee3119ff9933600d55d83ab4e6f69d481819aa920d14d`, final candidate `4f8ac38f6615c4ce5fdcc52f198b19717f4a6e954b4225e84ded5c2cec2778a5`. `.gitignore` excludes `candidate-bin/`.

## Change and correctness

Default normalized Markdown, HTML and XML values now call the concrete public `RenderedRepresentation::render` contract at a logical value boundary. Raw text and unchanged plain values retain borrowed paths; plain text needing normalization keeps its existing concrete route. The visible-text renderer normalizes directly into the reusable output, removing an intermediate normalized string. No callback enters the per-character scanner or bigram loop.

`run_paired_fts_recovery.py` completed five alternating process pairs at both 0 and 64 MiB query-cache budgets, seven measured query rounds per process. Every one of the 30 case signatures matched for ID, kind, query, field, type, match count, first ID, fields and matched paths. Every paired case also matched on candidates, scans, parses, cache hits and ordered hits; setup index sizes were identical. Raw stdout, stderr, commands, exit statuses, process manifest and all case summaries are in `paired-final/`. The complete per-case timing table is [PER_CASE.md](PER_CASE.md).

The final workspace test and Clippy commands passed. The core `index-timings` library tests passed (135 tests). Existing external consumer tests exercised built-in Markdown/HTML/XML registration against default matching, including raw and regex modes, candidates, refresh and snapshot restore. The workspace spacing check still reports seven previously recorded hunks in five untouched files; neither changed Rust file appears. Direct `rustfmt --check` reports the repository's intentional extra blank-line style. Logs are saved here. User-owned `docs/impl-plan.md` and `paused-stage2/BLOCKER.md` were left untouched.

## Paired timing and work

| Measurement | Reference median | Candidate median | Candidate change | Samples |
| --- | ---: | ---: | ---: | --- |
| FTS4 build, production allocator, no stage timers | 1,552.506 ms | 1,527.272 ms | -25.234 ms (-1.63%) | 5 paired independent processes, one warmup each |
| FTS4 build, counting allocator, no stage timers | 3,024.67 ms | 3,030.37 ms | +5.70 ms (+0.19%) | 5 paired independent processes, one warmup each |
| Counting build allocations | 26,827,288 | 26,614,165 | -213,123 (-0.79%) | Same counted processes |
| Counting build allocated bytes | 5,033,617,130 | 4,987,360,304 | -46,256,826 (-0.92%) | Same counted processes |
| Counting build peak live bytes | 234,956,394 | 234,631,236 | -325,158 | Same counted processes |
| Counting build peak RSS | 666,552 KiB | 666,952 KiB | +400 KiB | Same counted processes |
| Compressed FTS4 index | 47,456,174 bytes | 47,456,174 bytes | 0 | Every build process |

The production build comparison used `run_paired_index_build.py --mode A --skip-snapshot`; the counted comparison used the same driver with `--mode C --skip-snapshot`. Both used the existing four-field Musenalm FTS4 profile. Raw results are in `paired-index-build-a-final/` and `paired-index-build-c-final/`. Production build samples were reference 1,544–1,565 ms and candidate 1,521–1,539 ms. The final counted timing is effectively equal, while counts and cumulative bytes fell. Two earlier counted series, before the final plain-path adjustment, alternately favored reference by about 200 ms and candidate by about 108 ms; their raw results are preserved as `paired-index-build-c/` and `paired-index-build-c-repeat/`. Counting allocator wall time is therefore treated as a diagnostic, not a production latency estimate.

At cache 0, the largest positive paired median among the 30 FTS cases was +1.416 ms on the 176.178 ms regex case 29. Cases 9 and 10 were consistently about +1.168 and +0.898 ms on 113.477 and 110.476 ms references. At cache 64, the largest positive paired median was +0.103 ms on the 4.259 ms case 30. The full paired spread and all negative deltas are in `PER_CASE.md`; these small increases remain visible rather than being averaged away. There was no 80–100 ms default-query slowdown. Per-case work counters remained equal.

## Boundaries

The failed first production build invocation is retained in `paired-index-build/`: this driver requires `--skip-snapshot` without `index-timings`, so it measured no valid build. The earlier FTS and production-build runs against the first candidate are retained as `paired/` and `paired-index-build-a/`, with their preserved first-candidate binaries. The final-source results are the `*-final` directories.

This stage measured default FTS matching and the affected FTS4 construction path. It did not repeat the original plan's full mixed-query, cache-sequence, sort, grep, encoding, all-corpus build/restore or lifecycle matrices. Those final acceptance items must be assessed against the pinned original baseline and latest implementation before declaring the entire library migration complete. The explicitly registered path's large-corpus timing was measured in earlier stage reports, while this stage's external consumer tests establish result and candidate parity.
