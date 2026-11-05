# Text-search extension integration: focused comparison

## Revisions and method

- Starting production revision: `56262831c3ef50f9737951dbe4f044b2fdcf95ef` (`master`). Final production revision: `6997ec607b3576b76723e7575e1f6d743a276a91`. Driver revision is the final revision; the reference executable was preserved before production edits. The baseline manifest and raw reference output were committed before those edits.
- Production changes: `05137e5305` binds registered text definitions to ordinary FTS/JSON queries and engine-owned representation indexes; `41b395bec1` removes a redundant selected snapshot during explicit representation builds; `f0f2278c84` preserves explicit syntax and case overrides; `6997ec607b` applies syntax before legacy pattern compilation. Documentation is in `9855da668f`; benchmark-driver changes are `57f74e34db`, `2b189518cb`, and `82edc4c37f`.
- Host: Intel Core i9-13900K, Linux 7.2.13, Rust/Cargo 1.99.1, `RUSTFLAGS=-C target-cpu=native`, `RAYON_NUM_THREADS=16`, `taskset -c 0-15`, release profile, ASCII bigrams, `MetadataOnOpen`. The reference and candidate read the same `examples/musenalm` source paths with warm filesystem cache. The corpus has 148,486 records and 193,297,230 record bytes; its source-content/stat fingerprint is `90e36ad63dcb6386e9521c583fdcd36451fd3d6182187dced542fb8292f730ce`. Lockfile SHA256 is `2e219967e2327f963a8a7863bc649fdcccaf52b555de336604a810d72f003e52`. Full provenance is in [manifest.json](manifest.json) and [corpus-current.json](corpus-current.json).
- Reference binary SHA256: `aa49dd9f6540410e70d0946a64390c82bad2605ce521200eac1536b2c84e0fda`; final frontend binary: `adc9b166c527a15aac5e25b62a6ff2a375c6980067e043789e9416fc511a7344`; focused binary: `960013e19ff2aed85822ab0916f23ae4826f667539749ed8ddcfde7d1db82444`. Preserved binaries live under `target/benchmarks/text-search-extension-integration-20261105/` and are intentionally outside Git.

Exact commands, from the repository root:

```sh
RUSTFLAGS='-C target-cpu=native' cargo build --locked --release -p objekte-extension-tests --example frontend_index_compare --example text_search_integration_bench -j 2
RAYON_NUM_THREADS=16 taskset -c 0-15 target/benchmarks/text-search-extension-integration-20261105/reference/frontend_index_compare examples/musenalm 7 > crates/bench/results/text-search-extension-integration-20261105/reference-frontend.json
RAYON_NUM_THREADS=16 taskset -c 0-15 target/benchmarks/text-search-extension-integration-20261105/candidate/frontend_index_compare examples/musenalm 7 > crates/bench/results/text-search-extension-integration-20261105/candidate-frontend.json
RAYON_NUM_THREADS=16 taskset -c 0-15 target/benchmarks/text-search-extension-integration-20261105/candidate/text_search_integration_bench 0 7 > crates/bench/results/text-search-extension-integration-20261105/focused-cache0.ndjson
RAYON_NUM_THREADS=16 taskset -c 0-15 target/benchmarks/text-search-extension-integration-20261105/candidate/text_search_integration_bench 64 7 > crates/bench/results/text-search-extension-integration-20261105/focused-cache64.ndjson
```

The stderr stream is retained in adjacent `.stderr` files. All invocations exited successfully. The build repeat used the same frontend command with the final argument `1`, in reference/candidate/candidate/reference/reference/candidate order; its six raw files are named `final-build-repeat-*`.

## Existing frontend control

The same complete ordered 284 IDs, count, truncation, source-work counts, and 6,279,015 snapshot bytes were observed in both revisions. These timings are independent source-execution controls, not a sum of unrelated cases. Medians use seven rounds; spread is the observed min–max.

| Route | Reference median ms (min–max) | Candidate median ms (min–max) | Δ ms | Δ % | Reference → candidate work |
| - | -: | -: | -: | -: | - |
| Exact scan | 3161.874 (3151.337–3188.543) | 3180.039 (3153.830–3339.354) | +18.166 | +0.57% | [[133949, 133949, 0]] → [[133949, 133949, 0]] (reads, parses, candidates) |
| Indexed | 9.481 (9.033–9.587) | 9.505 (9.076–9.554) | +0.024 | +0.25% | [[326, 326, 325]] → [[326, 326, 325]] (reads, parses, candidates) |

The final build sample was 3823.278 ms reference versus 3855.418 ms candidate (+32.140 ms, +0.84%). A prior candidate revision produced a suspicious +167.514 ms sample, so alternating reference/candidate repeats were made before the final syntax-only patch:

| Process | Revision | Build ms | Restore ms |
| -: | - | -: | -: |
| 1 | reference | 3962.725 | 505.447 |
| 2 | candidate | 3842.666 | 509.450 |
| 3 | candidate | 4020.299 | 507.639 |
| 4 | reference | 3808.486 | 505.679 |
| 5 | reference | 3826.645 | 502.255 |
| 6 | candidate | 3872.556 | 638.798 |

Repeated build medians at the immediately preceding production revision (`f0f2278c84`): 3826.645 ms reference, 3872.556 ms candidate, Δ +45.911 ms (+1.20%). The pairs reverse direction and both revisions vary by over 150 ms, so the initial +167.514 ms was not reproducible. The final revision builds in 3855.418 ms (+32.140 ms) and restores in 510.868 ms (+2.334 ms) against the original reference samples. Snapshot bytes match exactly. No unexplained systematic source work increase appeared.

## New ordinary text-definition paths

The focused disposable fixture has two 512-record JSON collections and one Markdown-provider record, fixed source modification times, declared custom/default definitions, and warmed indexes. Each process emits 12 complete direct-versus-indexed signatures and 13 cases × seven query rounds. The driver asserts complete text IDs/fields/paths/definitions and JSON IDs/data, with counts and ordering for both; cache 0/64 signatures match with SHA256 `714c5734f21a902e1f50504b73e7249aa0f556fca2b48c09b87dd2d840863f9e`. These new request semantics have no equivalent reference recording, so the two cache columns below compare candidate configurations, **not** reference and candidate speed.

| Case | Total | 0 MiB median ms (min–max) | 64 MiB median ms (min–max) | 0 MiB candidates / reads | 64 MiB candidates / reads |
| - | -: | -: | -: | -: | -: |
| `custom_absent` | 0 | 2.650 (2.627–2.696) | 0.015 (0.014–0.023) | 382 / 382 | 0 / 0 |
| `custom_and` | 1 | 0.075 (0.067–0.082) | 0.046 (0.041–0.049) | 1 / 2 | 0 / 1 |
| `custom_broad` | 131 | 1.038 (1.026–1.055) | 0.104 (0.100–0.161) | 131 / 141 | 0 / 10 |
| `custom_direct` | 3 | 3.636 (3.568–3.680) | 0.049 (0.044–0.056) | 513 / 516 | 0 / 3 |
| `custom_native_direct` | 2 | 3.516 (3.481–3.520) | 0.042 (0.041–0.046) | 512 / 514 | 0 / 2 |
| `custom_native_index` | 2 | 0.084 (0.082–0.087) | 0.044 (0.042–0.050) | 2 / 4 | 0 / 2 |
| `custom_or` | 1 | 3.625 (3.580–3.655) | 0.038 (0.035–0.041) | 513 / 514 | 0 / 1 |
| `custom_selective` | 3 | 0.078 (0.063–0.094) | 0.043 (0.041–0.172) | 3 / 6 | 0 / 3 |
| `custom_sorted` | 131 | 1.147 (1.125–1.164) | 0.105 (0.101–0.114) | 131 / 141 | 0 / 10 |
| `custom_stream` | 3 | 0.092 (0.091–0.094) | 0.071 (0.069–0.078) | 3 / 6 | 3 / 6 |
| `native_fts` | 2 | 0.081 (0.076–0.094) | 0.033 (0.031–0.040) | 0 / 4 | 0 / 2 |
| `native_scalar` | 1 | 0.428 (0.423–0.434) | 0.420 (0.413–0.458) | 0 / 1 | 0 / 1 |
| `second_definition` | 1 | 0.051 (0.050–0.053) | 0.029 (0.028–0.040) | 1 / 2 | 0 / 1 |

The 64 MiB run reuses completed ordered results for collected cases and reads only returned pages. The streamed case still verifies candidates. The native scalar case uses its native path and is not a selected ordered-cache hit. The cache-off selective custom path visits 3 candidates and reads 6 sources, while direct execution visits 513 candidates and reads 516 sources; broad and absent terms show their actual work. Cache-off `native_fts` uses 2 native FTS candidates and 4 reads. Focused build/restore are single setup samples: 73.98/41.71 ms at 0 MiB and 58.20/77.99 ms at 64 MiB. Snapshot bytes were 809,195/809,339, respectively; distinct disposable paths and metadata mean they are not a size regression comparison. No peak RSS or allocation profile was recorded for this bounded check.

## Correctness and architecture

- `TextSearchDefinition` composes one registered representation and exact operators under a stable canonical identity and optional aliases. Binding resolves the field default or explicit selector, compiles once, and records semantic identities. The same bound representation and scanner verify native or selected records; the engine owns ordinary/skip-one bigram postings, typed conditions, ordering, and caches.
- `x-text-search` declares field defaults and eligible indexes; legacy `x-full-text` maps to built-in definitions. `TextQuery.fields` accepts strings or typed `{path,search,syntax,case_sensitive}` selectors. JSON filters accept aliases and `:text("canonical:id")`; plain `:fts` resolves the configured default. Explicit target settings precede request settings, legacy mode/case, then operator defaults. `Some(false)` is distinct from an omitted case setting.
- Ordinary native JSON keeps its concrete path when no bound definition is needed. Bound definitions can use selected representation postings in native-only and mixed collections, with exact verification and safe fallback for missing/incompatible constraints. The public example `crates/extension-tests/examples/text_search_definition.rs` registers an abbreviation representation on JSON strings without a provider or frontend; its `MS` source matches `manuscript` only after transformation.
- The definition-aware build uses the existing fused selected projection builder; low-level representation builds no longer create an extra selected source snapshot. Save/reopen and refresh retain definition identity and source generation. Candidate pruning requires compatible representation and explicit error-safe skip permission. Direct or disabled-index execution remains an exact scan.

## Validation and limitations

- `cargo test --locked --workspace --all-targets -j 2`: passed. `cargo clippy --locked --workspace --all-targets -j 2 -- -D warnings`: passed. `cargo run --locked -p objekte-extension-tests --example text_search_definition -j 2`: passed. Seven public consumer text-integration tests passed, including registration conflicts, defaults, built-in markup parity, transformed-output candidate pruning, mixed sources, arrays, errors, streaming, cache, save/reopen, and refresh.
- Changed Rust files received rustfmt followed by the repository spacing transform; `git diff --check` passed. The repository-wide `python3 scripts/format-rust-spacing.py --check` still fails on untouched files already inconsistent with rustfmt/spacing; no unrelated whole-workspace reformat was made.
- A one-off explicit definition without a declared/warmed index scans exactly. Short, broad, incompatible, or unsafe constraints may also scan; index presence never changes interpretation. A custom representation with no native-projection equivalence uses its own selected posting domain. Sorted searches may prepare verified candidates before returning a page. No dynamic plugin loading or new index storage was added.
- Seven rounds do not establish stable tail latency or universal zero overhead. The native control covers one existing frontend workload; other corpus/encoding combinations were outside this focused task. Result-cache hits measure page retrieval, not scanner speed; the 0 MiB figures and candidate counters establish scanner/index work.

Raw data: [reference](reference-frontend.json), [candidate](candidate-frontend.json), [focused 0 MiB](focused-cache0.ndjson), [focused 64 MiB](focused-cache64.ndjson), [machine-readable summary](summary.json). Earlier candidate and alternating runs are retained with `before-*` and `repeat-*` names.
