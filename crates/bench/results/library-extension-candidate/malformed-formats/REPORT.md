# Recoverable malformed third-party formats

## Revisions and reference

The clean starting checkout was `c33898bb28ba6ece25b257345021f36ec3b0ba2b`; its last production change was `3136fa578b65b838611f09d3025927de9aaad7ce`. Baseline provenance was pinned in `0a5db75cdd`. The delivered production change is `03e95abce27979fe60e24e83371fa6c38781e54d`. No production code changed after its release build. The [baseline](baseline-manifest.json) and [candidate](candidate-manifest.json) manifests identify the exact binaries, toolchain, inputs, corpus fingerprint, and settings. The reference binary (`93fc123eeeec5ad18e081bca60865ccb20d58aaa4a068c1d303deb2ac6185c7b`) and candidate binary (`874ac82933d3f519ebf576091fc58872a64b3a24859e942f342a03d0595888a2`) are saved locally in ignored `reference-bin/` and `candidate-bin/` directories so builds cannot overwrite them.

The reference raw results are the immediately preceding [review-findings candidate run](../review-findings-1-3/candidate/). Reuse is valid here because its production revision and preserved binary match the starting checkout, and the host, compiler, configuration, workload, corpus content and filesystem-stat fingerprint, affinity, worker count, and release options match. The candidate [corpus fingerprint](candidate-corpus-fingerprint.json) confirms 148,486 records, 193,297,230 bytes, and source/content/stat SHA-256 `90e36ad63dcb6386e9521c583fdcd36451fd3d6182187dced542fb8292f730ce`. The loaded workload has **152 cases** and SHA-256 `5cfb37386221a6f2237bc1755e12c7a89f1e1f4fdeff4dd57f1b00370c4eb99e`.

## Implementation and tests

`FormatDecoder::decode_outcome` is an additive entry point. Its default delegates to the legacy `decode`, so legacy errors remain errors. A new decoder reports `Decoded` or `Malformed(Vec<FormatDiagnostic>)`; the latter supplies stable code, message, and optional validated half-open source byte span. `malformed_text` is an explicit, default-disabled fallback capability. The built-in JSON decoder uses one Sonic parse and the existing JSON malformed-text representation. The downstream `ArticleDecoder` example returns borrowed UTF-8 source text only when it can decode it; its binary/invalid-UTF-8 cases cannot fabricate a text hit.

The core `decode_checked` adapter enforces exactly one callback for success, none for malformed content, and precedence of the first callback error even if a decoder attempts to hide it. High-level selected record access retains source identity, metadata, exact original bytes, `incorrect_format` (or `incorrect_json` for built-in JSON), and source diagnostics. A malformed record's null `data` is only the existing compatibility placeholder: schema validation, logical field representations, typed postings, and derived extractors do not treat it as a decoded null. Low-level methods requiring a logical record return a typed malformed-source error. Successfully decoded records keep the existing schema policy and logical indexed execution.

Selected text verification calls the decoder's fallback only for confirmed malformed sources and only for unscoped built-in text matching; it returns the existing `$raw` marker. Candidate selection conservatively includes the generation's malformed slots alongside logical-text postings. Incremental reuse remaps unchanged malformed members and overlays changed records. Fused selected builds classify once during the normal decode, omit logical contributions for malformed sources, and record typed missing rows. Selected text snapshot companion metadata is version **6**, persists malformed membership, and rejects older or incompatible manifests. Native JSON snapshot bytes and concrete native paths remain unchanged. Standard refresh, save/reopen, and source-generation invalidation continue to publish compatible inventory, postings, and ordered results.

The [external consumer tests](../../../../extension-tests/src/malformed_formats.test.rs) cover callback contract violations, callback and fallback errors, diagnostics and spans, schema separation, raw word/word/regex searches, scoped nonmatches, metadata/status/Boolean filters, JSON parity through a custom provider, root null and arrays, invalid bytes, disabled fallback, valid↔malformed edits, rename/delete, derived and text index updates, snapshot restore and version rejection, selective native work, and streaming callback failure. `cargo test --locked --workspace --all-targets -j 2` passed, including all 59 extension-consumer tests; see [test log](workspace-tests.log). `cargo clippy --locked --workspace --all-targets -j 2 -- -D warnings` passed; see [Clippy log](clippy.log). Changed Rust files were formatted and `git diff --check` passed. The repository-wide [spacing check](format-check.log) still exits 1 at seven pre-existing locations; none is in the new malformed-format implementation or test file.

## Native 152-case comparison

Both revisions used `cargo build --locked --release -p objekte-bench --bin json-query-mix -j 2`, configured schemas, ASCII bigrams, seven rounds, and `RAYON_NUM_THREADS=16` pinned to CPUs 0–15. The reference invocation was:

```sh
env RAYON_NUM_THREADS=16 taskset -c 0-15 python3 crates/bench/run_library_query_matrix.py \
  --bin-dir crates/bench/results/library-extension-candidate/review-findings-1-3/candidate-bin \
  --output-dir crates/bench/results/library-extension-candidate/review-findings-1-3/candidate \
  --runs 7 --only json-schema-ascii-cache0,json-schema-ascii-cache64
```

The candidate invocation was:

```sh
env RAYON_NUM_THREADS=16 taskset -c 0-15 python3 crates/bench/run_library_query_matrix.py \
  --bin-dir crates/bench/results/library-extension-candidate/malformed-formats/candidate-bin \
  --output-dir crates/bench/results/library-extension-candidate/malformed-formats/candidate \
  --runs 7 --only json-schema-ascii-cache0,json-schema-ascii-cache64
```

The runner's [reference commands](../review-findings-1-3/candidate/commands.json) and [candidate commands](candidate/commands.json) record each binary's exact arguments, exit status, and invocation times. The full [per-case comparison](comparison.json) and [candidate raw rows and stderr](candidate/) are preserved. Every case has the same match count and first ID; source reads, parses, candidate scans, predicate-cache hits, and ordered-cache hits match **exactly** at both budgets. The benchmark records only count and first ID signatures, so the focused tests establish complete-result behavior for new formats.

| Query cache | Reference sum of case medians | Candidate sum | Difference | Largest slower case |
| --- | ---: | ---: | ---: | --- |
| 0 MiB | 10,901.82 ms | 10,863.42 ms | −38.40 ms (−0.35%) | Case 116: +4.98 ms (+0.66%) |
| 64 MiB | 9,950.39 ms | 9,891.06 ms | −59.33 ms (−0.60%) | Case 88: +2.32 ms (+0.85%) |

These are sums of per-case medians, not end-to-end elapsed time or tail estimates. At 0 MiB, only three cases increased by at least 1 ms and none by 5 ms; at 64 MiB, the same counts apply. Case 116's reference and candidate seven-round ranges are 744.87–755.05 and 749.85–785.77 ms, respectively; its median shift is 0.66%. Case 88's ranges are 268.56–277.88 and 272.52–275.49 ms. Neither represents a plausible material regression under the agreed procedure, so no alternating repeat was needed. Small percentage changes on short cases do not show changed work. The native benchmark protects the native route; it does not measure custom-format throughput.

## Limits

Malformed-source full-text verification initially scans the malformed subset. Many malformed sources can therefore make unscoped text searches slower; no new raw-source index was added. A format without `malformed_text` remains visible through record and metadata access but has no raw text hit. Custom operators do not automatically participate in raw fallback because their declared logical representation need not match original source text. Request-time freshness retains its documented source validation work. Legacy decoders need no source change but must override `decode_outcome` to opt into recoverable syntax errors; a decoder that may discover syntax failure after invoking its callback must keep reporting that late failure as an operational error.
