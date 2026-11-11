# Candidate verification optimization results

Task: `candidate-verification-optimization`
Date: 2026-11-11
Base: `2bd4dbb15fd6bc7ddec178d056eefcad9b764362`
Final implementation: `97f3d75d1189ae025fa30289424a40d4af0e20e3`
Branch: `integration/candidate-verification-optimization`

## Result

Indexed built-in JSON, decoded-scalar JSON, and JSONL records now use the validation classification published by explicit build/update operations. A logical record uses trusted query extraction; a raw fallback keeps its recorded fallback behavior. Extension readers and unindexed scans remain validating. Search no longer compares source metadata, revisions, or fingerprints, or requests reconciliation because bytes changed externally. Explicit lifecycle operations still validate newly indexed content and publish the resulting classification. No snapshot format, query syntax, default query behavior, or source corpus changed.

The decoded-scalar reader extracts requested logical values and scalar text in one structural walk. Trusted FILTER-only extraction skips unrequested values without validating their content or trailing input. Trusted logical output applies schema conversions without repeating declared-type validation. Required string tokens are decoded with serde's streaming string deserializer and its byte offset; this avoids a second scan of escaped strings while leaving whole-document validation to explicit index operations and unvalidated readers.

Prepared literal FILTER and eligible literal FTS matching use owned `memchr::memmem::Finder` values. Case folding reuses bounded worker-local scratch storage and preserves the existing Unicode path. The JSON walker uses `memchr2` to find the first quote or backslash, then a byte loop after an escape.

## Correctness and validation

- `cargo test --workspace`: passed on the final implementation, including lifecycle, relation, streaming, malformed-input, source-range, conversion, parser, and matcher tests. The core library had 489 tests, of which 12 were intentionally ignored.
- `cargo clippy --workspace --all-targets`: passed with existing workspace warnings. Strict `-D warnings` remains unsuitable for this branch because it reports numerous warnings in untouched files.
- The repository spacing formatter was applied to task-owned Rust files; unrelated formatter edits were restored. Task-owned Rust files pass the spacing check and `git diff --check`.
- All eight focused corpora have identical complete membership SHA-256 values and source-work counters between base and final runs. The short-plain corpus's 14,757-line oracle for ordered hits, counts, evidence, diagnostics, and locator/byte/logical payloads is byte-for-byte identical: SHA-256 `29a8cf0c32f57df83f9d8ab855034d49300a35234b00f122e323913bcf6fd0ce`.
- Both full Musenalm suites passed all 388 case/profile rows each (194 queries under ASCII and Unicode). Baseline and final CSVs have zero differences in status, result counts, candidate/read/parse counters, or cache-hit counters. The suites exercise the fixed workload's FTS, FILTER, relation, sort, and cache paths.
- Focused tests confirm that a built-in indexed logical record takes trusted extraction, an extension registration takes validating extraction, malformed raw fallbacks do not revalidate during search, and an explicit update changes the saved class. External source edits may now yield stale bytes or an extraction error until that update; this is the specified policy.

## Benchmark setup

All measurements used the same Fedora x86-64 machine (Intel Core i9-13900K), Rust/Cargo 1.99.1, locked release builds, and four query workers. Focused searches disabled the verified-result cache, warmed the OS file cache, and used seven timed repetitions after warm-up. The Amazon sample is the first 20,000 local review files. Long, duplicate-heavy, and deep fixtures are deterministic synthetic controls. The full Musenalm suites used five repetitions per query/profile. Exact commands, fixture digests, raw samples, manifests, and comparison TSVs are in [`bench/results/candidate-verification-optimization/`](../../../bench/results/candidate-verification-optimization/README.md).

The focused command shape was:

```sh
cargo build --release --locked -p objekte-search-core --example bench_candidate_verification --example bench_musenalm
./target/release/examples/bench_candidate_verification --root /tmp/objekte-candidate-verification-amazon20k --repetitions 7 --workers 4
python3 bench/compare_candidate_verification.py bench/results/candidate-verification-optimization/base-amazon20k.jsonl bench/results/candidate-verification-optimization/integration-amazon20k.jsonl
```

### Combined focused results

Median warm repository search time in milliseconds, base → final:

| Corpus | Decoded-scalar FTS `good` | `helpful_vote > 3 && rating = 5` | FTS plus FILTER | `text ~ 'good'` |
| --- | ---: | ---: | ---: | ---: |
| Amazon 20k | 13.526 → 9.444 | 17.802 → 15.857 | 9.877 → 7.430 | 18.809 → 17.336 |
| Long plain, synthetic | 4.271 → 3.135 | 2.141 → 1.770 | 3.048 → 1.701 | 1.988 → 2.286 |
| Long escaped, synthetic | 28.737 → 13.841 | 12.023 → 5.171 | 27.904 → 12.956 | 13.937 → 13.086 |
| 256 duplicate keys, synthetic | 8.937 → 7.830 | 0.908 → 0.827 | 8.710 → 7.397 | 0.773 → 0.907 |
| Deep 112, synthetic | 12.982 → 10.870 | 0.688 → 0.489 | 12.948 → 10.396 | 0.588 → 0.551 |

The 0.13–0.30 ms FILTER-contained differences on the long-plain and 256-duplicate synthetic fixtures varied across repeated runs. They are the only final focused searches more than 5% slower in the last seven-repetition run; their absolute differences are 0.298 and 0.134 ms. Amazon and long-escaped FILTER-contained searches improved after the accepted string-token change.

In-memory decoded-scalar FTS extraction plus evaluation, excluding source I/O and repository planning, changed from 68.641 to 48.357 ms on Amazon 20k and from 142.845 to 62.399 ms on long escaped records. Gross allocated bytes fell from 192.8 to 117.9 MB and from 185.4 to 114.5 MB, respectively. These rows compare the base validating path with the final trusted indexed path, as labeled in the JSONL; the validating-path rows are retained separately.

### Full-suite and build controls

The median of 388 final/base per-query time ratios was 0.912 in the uncached suite and 0.995 in the warm-cache suite. The fixed Q001 unscoped FTS query took 37.326 → 30.624 ms under uncached ASCII. Warm-cache microsecond results mostly reflect cache lookup and planning; they are a control, not evidence of faster candidate verification.

Eight uncached Musenalm case/profile rows above 1 ms showed more than a 5% regression in the final five-repetition suite; all differences were under 0.8 ms. Five representative cases were rerun in alternating base/final order, twice each, with nine repetitions. For example, Q164 ASCII changed from 7.77 to 7.09 ms in the repeated comparison, while Q166 ASCII was 14.79 to 15.65 ms; its two base medians ranged from 13.60 to 15.97 ms. The raw repeat samples and summary TSV are retained. Warm-cache rows above 1 ms had no regression above 5%.

Fresh Musenalm builds parsed the same 148,486 records and produced identical estimated index bytes for each alphabet. Across the two base and two final build runs, discovery and extraction times varied in both directions; there was no repeatable build regression. The build parser remains the validating path.

## Variant decisions

| Change | Independent observation | Decision |
| --- | --- | --- |
| Combined decoded-scalar query extraction | Parser-only in-memory FTS: Amazon 70.4 → 51.5 ms; long plain 11.1 → 8.5 ms; long escaped 140.6 → 79.6 ms, with matching membership. | Keep. |
| Published validation dispatch plus search read policy | At intermediate revisions `b4595ea083` → `525ef56135`, long-escaped FILTER votes/rating warm search was 12.716 → 5.669 ms; Amazon FILTER contains was 20.439 → 18.178 ms. Long-escaped FILTER contains regressed 13.860 → 15.023 ms before string-token improvement. | Required policy; keep and address the regression. |
| Owned Finder and reusable folding scratch | Release microbenchmark: 8 KiB short-needle late/miss 147–149 → 117–121 ns; wildcard miss 1,135 → 117 ns; 96-byte ASCII fold plus match 931 → 22 ns. Unicode 1,500-byte folding was 14,550 → 14,482 ns. Finder preparation medians were 16 ns (3-byte literal), 53 ns (29-byte literal), 102 ns (5-byte wildcard), and 204 ns (29-byte wildcard). | Keep; preparation is amortized by candidate reuse. |
| Hybrid JSON delimiter search | Against parser-only, long-plain in-memory FTS 8.53 → 4.08 ms, long escaped 79.56 → 73.43 ms, Amazon 51.54 → 49.59 ms. | Keep. |
| Direct repeated `memchr2` | Long-escaped in-memory FTS 79.56 → 120.35 ms. | Reject. |
| Quote-only `memchr` | Long-escaped in-memory FTS 79.56 → 84.98 ms. | Reject. |
| Plain-string and key-only trusted fast paths | Exploratory long-escaped FILTER-containing warm searches reached 17.42 and 17.87 ms versus a 13.94 ms base and 15.06 ms adjacent committed-code run. | Reject; gains were not broad. |
| One-pass required string token decoding | Long-escaped FILTER-containing warm search returned to 13.086 ms; Amazon improved to 17.336 ms. Parser and oracle parity held. | Keep. |

These variant measurements have different code dependencies and timing scopes. Their percentage changes must not be added together. Intermediate and exploratory raw JSONL files are under `experiments/`; the final combined result is in `integration-*.jsonl`.

## Boundaries and remaining work

Indexed valid built-in records have no whole-document or schema validation pass during search. Token decoding and structural work needed for selected values, duplicate-member resolution, FTS evidence, and requested payloads remain. The search call chain uses open/read/range checks without metadata or fingerprint probes. Exact-index and verified-cache paths avoid source reads when no payload or residual evaluation needs them. Unindexed and extension-reader paths still validate their query input.

The optional restricted early-exit verifier was not retained. Its safe eligibility would need to cover duplicate keys, evidence, representation transforms, relation evaluation, schema conversion, sorting, and logical payloads; the representative Amazon end-to-end gain from the lower-risk combined path already reduced the remaining verification window to single-digit milliseconds. A future prototype should start with literal unscoped FTS and mostly rejected candidates, report the eligible fraction and full-query gain, and keep the existing evaluator for all unsupported shapes. No `sonic-rs` or `simd-json` parser substitution was made in this task.

Under the chosen consistency model, an external edit can produce a stale candidate, stale cached answer, changed byte payload, missed match, or extraction error until an explicit index operation updates it. Runtime I/O errors, range checks, cancellation, and resource limits remain in place. No unresolved in-scope correctness failure or benchmark gate remains.
