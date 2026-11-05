# Direct candidate eligibility and native translation

## Scope and revisions

The clean starting checkout and production revision was `3014c314c691c61d46a9604efee4212713ed97ed`. The prior 152-case native benchmark is not a baseline for either mixed-source path, so a focused public-query benchmark was added before production edits in `e32c2671fa`. Its ordinary release binary is preserved locally as `reference-bin/mixed_candidate_paths` (SHA-256 `f5cda053e8489c334254d53cce399b3445fcdeb228437284f20c016aad189acd`). A diagnostic-only change in `3b503bd634` recorded native-order entries, bitmap word lookups and translated native slots without changing the selection algorithm; its reference binary is `reference-bin/mixed_candidate_paths_instrumented` (SHA-256 `39de5a040e8165c9bf341c70056cab7f4a2d323f47762682ab674113289e9ff7`). The [baseline manifest](baseline-manifest.json) and both sets of raw rows ([ordinary](baseline/), [instrumented](baseline-instrumented/)) preserve provenance.

The new downstream tests were committed in `a675448d1c`. They both failed on the instrumented, unfixed implementation: `Direct` visited **65 instead of 97** selected records, while a rare dense FTS selection visited **128 native-order entries** to translate three native candidates. The [pre-fix failures](pre-fix-regressions.log) record the assertions. The fixes are `334a3040f4` (Direct backend eligibility) and `1a06d66436` (unordered translation). The benchmark driver received formatting only in `eed7c96759`; that is the exact candidate checkout. The final production revision is `1a06d66436`. The candidate binary is preserved locally as `candidate-bin/mixed_candidate_paths` (SHA-256 `5765fa2d5ab3ae554d86f6a3ce86476cfbeca6aaf6027273add82142b787fe2a`). Binaries remain ignored by Git; raw data and metadata are committed.

## Changes and correctness

`query_json_selected` now defines one `may_use_candidates` predicate for `Auto` and `Bigram`. Its initial selected-field lookup, native lookup, and selected-field fallback all use that predicate. `Direct` does not consult the selected-field estimate or prune through typed, presence, scalar, or text postings. `Auto` and `Bigram` retain the fallback and its compatibility and cardinality safeguards. A compatible ordered-result cache may still satisfy a request when query caching is enabled; the regression disables verified-result caching while retaining warm indexes to test physical execution.

`SlotSelection::visit_translation_members` enumerates sparse stored slots or dense bitmap set bits without native result order. Empty membership returns immediately; unrestricted membership remains distinct. The generation-validated native-to-selected map, custom fallback slots, final selected ordering and pagination are unchanged. Dense translation examines up to one bitmap word per 64 native slots, plus its set bits, and writes a selected-domain bitmap; it is **not** O(matches). Sparse translation visits its stored members. A slot-layout change can still rebuild the map in O(native records) once for that layout. The repeated-query benchmark starts after that map is valid.

The public [mixed-source regressions](../../../../extension-tests/src/review_direct_translation.test.rs) compare complete collection-qualified IDs, totals, truncation, pages, and a scalar-cardinality error with a separate direct scan oracle. They cover `Direct`, `Auto`, `Bigram`, empty FTS results, a dense FTS intersection in 128- and 1,024-native-record fixtures, and refresh plus compaction. The [internal membership test](../../../../core/src/file_picker/test.rs) covers sparse, dense and empty selections independently of final ordering. Existing mixed-index, candidate, compaction and malformed-source tests passed together. The full [workspace test log](workspace-tests.log) records 61 passing downstream tests; [Clippy](clippy.log) passed with warnings denied. Changed Rust files pass [rustfmt](rustfmt-check.log) and `git diff --check`. The repository-wide [spacing check](format-check.log) still exits 1 at the same seven pre-existing locations; none is in the new implementation or regression file.

## Focused release benchmark

The fixture generator and runner are [public consumer code](../../../../extension-tests/examples/mixed_candidate_paths.rs). Each fixture has one custom JSON-decoded `.md` source, a configured `body` full-text and scalar posting, a configured `year` posting, a common `:fts` term, and three rare native FTS matches. The small fixture has 256 native files; the large has 8,192. The exact source/content/stat fingerprints are in the [baseline](baseline-manifest.json) and [candidate](candidate-manifest.json) manifests and were checked unchanged after the candidate run. The host used Rust/Cargo 1.99.1, ordinary release settings, four Rayon threads pinned to CPUs 0–3, `query_cache_bytes = 0`, and nine rounds. Setup warms the selected and native indexes. The first rare FTS call initializes the native slot map; repeated calls use the valid map.

The build command on both revisions was:

```sh
cargo build --locked --release -p objekte-extension-tests --example mixed_candidate_paths -j 2
```

For each `phase` (`baseline`, `baseline-instrumented`, `candidate`) the exact run pattern was:

```sh
root=$(cat crates/bench/results/library-extension-candidate/direct-translation-20261105/fixture-root.txt)
env RAYON_NUM_THREADS=4 taskset -c 0-3 <phase-binary> run "$root/small" 9 > <phase>/small.ndjson 2> <phase>/small.stderr.log
env RAYON_NUM_THREADS=4 taskset -c 0-3 <phase-binary> run "$root/large" 9 > <phase>/large.ndjson 2> <phase>/large.stderr.log
```

`<phase-binary>` was the corresponding preserved executable named above. The [comparison](comparison.json) holds every repeated sample, initial-call time, signature, plan and work counter. The table compares the equally instrumented reference and candidate medians; the ordinary uninstrumented reference is also retained in the raw data. Every scenario retained its total, truncation and first ID. The tests establish complete ordered results.

| Native files | Path | Reference median | Candidate median | Delta | Candidate visits before → after | Native-order entries before → after | Bitmap words before → after | Native slots translated |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 256 | `Direct` scalar fallback | 1.312 ms | 1.953 ms | +0.641 ms (+48.8%) | 172 → 257 | 0 → 0 | 0 → 0 | 0 |
| 256 | Indexed scalar fallback | 1.320 ms | 1.326 ms | +0.006 ms (+0.5%) | 172 → 172 | 0 → 0 | 0 → 0 | 0 |
| 256 | Rare native FTS | 0.0595 ms | 0.0593 ms | −0.0002 ms (−0.4%) | 4 → 4 | 256 → 0 | 256 → 4 | 3 |
| 8,192 | `Direct` scalar fallback | 41.608 ms | 62.719 ms | +21.111 ms (+50.7%) | 5,463 → 8,193 | 0 → 0 | 0 → 0 | 0 |
| 8,192 | Indexed scalar fallback | 41.563 ms | 41.854 ms | +0.291 ms (+0.7%) | 5,463 → 5,463 | 0 → 0 | 0 → 0 | 0 |
| 8,192 | Rare native FTS | 0.114 ms | 0.105 ms | −0.009 ms (−7.9%) | 4 → 4 | 8,192 → 0 | 8,192 → 128 | 3 |

Source reads and parses match candidate visits plus the returned-page read in every row: the `Direct` increase is the required correction from indexed pruning to full-domain evaluation. Indexed scalar work is unchanged and its small latency changes lie within the nine-sample ranges. Rare FTS retains four selected candidates (three native plus one custom fallback) and five source reads on both revisions. Its microsecond timing change is too small and noisy to establish a general speedup; the deterministic traversal counters establish the eliminated native-order walk. The first rare FTS call, which builds the map, took 0.147 vs 0.150 ms for 256 files and 3.374 vs 3.296 ms for 8,192 files; that collection-sized setup remains separate from repeated translation. No full 152-query suite was rerun, as requested.

The remaining cost for repeated dense translation is the O(native bitmap words) scan, the selected-domain bitmap allocation, and final candidate ordering. A later compressed or sparse representation might reduce that further, but no such storage redesign is part of this fix.
