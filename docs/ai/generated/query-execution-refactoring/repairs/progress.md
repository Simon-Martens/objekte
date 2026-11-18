# Query execution repair progress

Task: `core-query-execution-repair`

Reviewed repair tip: `20e17c5be0d1f6c227503d5681c2bf7c76ca2808`

This record covers the repair work completed through commit
`a0e4f7b171`. The repair specification is **not complete**; pending R3–R8
items are listed below rather than represented as passing.

## Completed evidence

- R1 exhaustive four-slot subset oracle covers empty, all, borrowed, owned and
  shared representations with different narrowed inputs and all set operations.
- Domain identity now contains repository, collection, revision and
  base/segment identity without hash truncation. The reviewed XOR collision and
  each domain component are covered by tests.
- Provider restriction consumes effective members rather than the original
  input. Stable handles remain valid after later proposals, and wrong term or
  domain applications return checked errors.
- Gate A passes through `Repository::search` with independent title/year
  providers. Text-first receives `[0,3,4]` for the structured step; structured
  first receives `[1,3,4,6]` for text. Both return record 3 with identical
  evidence and count.
- Ordinary EXPLAIN performs zero provider applications. ANALYZE performs one
  instrumented search and records ordered provider observations with input and
  candidate counts for each physical operation.
- Two exact predicate implementations (bound matcher and byte-window evaluator)
  produce identical ordered results and evidence under term, AND and OR queries.

## Gate A observations

| Order | First input | First output | Second input | Second output | Final |
| --- | --- | --- | --- | --- | --- |
| Text first | `{0,1,2,3,4,5,6,7}` | `{0,3,4}` | `{0,3,4}` | `{3,4}` | `{3}` |
| Structured first | `{0,1,2,3,4,5,6,7}` | `{1,3,4,6}` | `{1,3,4,6}` | `{3,4}` | `{3}` |

## Benchmark evidence

- Host: AMD Ryzen 7 5700G, 32 logical CPUs
- Toolchain: Rust/Cargo `1.99.1`
- Corpus: `/home/simon/source/objekte/examples/musenalm`
- Workers: 4, package default features, fresh result directories
- All full-run result and work digests matched original master
- All incremental Unicode result and work digests matched original master
- No case crossed the 5% + 0.2 ms investigation gate

| Case | Base ms | Repair ms | Ratio |
| --- | ---: | ---: | ---: |
| Q001 | 15.757 | 16.363 | 1.0384 |
| Q007 | 364.666 | 356.140 | 0.9766 |
| Q030 | 2.993 | 3.082 | 1.0297 |
| Q035 | 0.053 | 0.054 | 1.0179 |
| Q084 | 0.037 | 0.039 | 1.0418 |
| Q127 | 0.068 | 0.068 | 0.9991 |
| Q141 | 4.623 | 4.550 | 0.9842 |
| Q150 | 2.701 | 2.803 | 1.0375 |
| Q163 | 2.233 | 2.166 | 0.9701 |
| Q164 | 7.129 | 6.415 | 0.8998 |
| Q181 | 35.182 | 35.033 | 0.9958 |

Incremental Unicode (`unicode-off` / `unicode-warm`, median ms):

| Case | Base | Repair |
| --- | ---: | ---: |
| Q030 | 3.431 / 0.005766 | 2.970 / 0.005910 |
| Q141 | 4.311 / 0.007758 | 5.049 / 0.008289 |
| Q163 | 2.267 / 0.005143 | 2.233 / 0.004847 |

## Pending repair requirements

- R3: route every remaining access family through the production catalog,
  remove conversion back to `AccessPathEstimate`, implement full adaptive
  rescoring, and remove specialized executor dispatch.
- R5: represent skipped nodes, cache-hit observations, combined FILTER/FTS
  physical-node identity and multi-domain parity completely; remove remaining
  post-search trace reconstruction.
- R6: eliminate `segment_expr_bounds` exact-set duplication, native stream
  `rows.to_vec()` copies, provisional-page winner clones, cached evidence
  clones, and remaining whole-input copies.
- R7: transfer source scanning, segment scanning, relations, ordered traversal,
  materialization and delivery algorithms from the 13k-line runner into the
  specified modules with production callers.
- R8: rerun the final core-only validation after R3/R5–R7 are complete and
  append the correction to the task document.
