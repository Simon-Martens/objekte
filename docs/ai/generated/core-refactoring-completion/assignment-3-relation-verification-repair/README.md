# Assignment 3 relation verification repair evidence

Tested repair source:
`fd25d8678af5161cb92647d7dbddefa7f1aff391`.

The production repair is in `ec12e0359c`; `fd25d8678a` makes the
test-only probe records identify successful execution explicitly.

## Artifact index

| Path | Contents |
|---|---|
| `a2-relation-probes.log` | A2 observations emitted by the existing deterministic semijoin and residual-sort fixtures |
| `final-relation-probes.log` | The same observations at the repair source |
| `a2-run-manifest.json`, `final-run-manifest.json` | Revisions, fixture settings, expected cases and repetitions |
| `relation-repair-comparison.json` | Asserted traversal and archived posting comparison |
| `verifier-self-test.log` | Valid control plus all five required negative controls |
| `q136/` | Focused alternating Q136 batches, per-commit narrowing and analysis |
| `final-*.log` | Final check, relation suites and workspace attempts |
| `desktop-open-{baseline,final}.log` | Reproduced pre-existing desktop test failure at B and final |
| `discovery-exclusion-{baseline,final}.log` | Isolated pass after the concurrent-suite fixture collision |

## Relation observations

The A2 and final observations match completely in ordered IDs, evidence,
selected route, candidate/proven membership, plan and every work field:

| Case | Route | Ordered result | Key work |
|---|---|---|---|
| Indexed relation FILTER | `relation_target_first` | `S00` through `S39` | One edge, one reverse posting, 40 reverse members, zero target reads, zero scans |
| Selected-field relation FTS | `relation source-restricted` | `S00`, fields `entry.title,title` | One edge, one target read/open, 56 relation bytes, zero scans |
| Source-dependent residual sort | `relation_target_first`, bounded top-k | `m8,m1`, equal to reference | Two sort-target reads, two target reads/opens, 93 relation bytes, one reused target, nine residual evaluations |

The archived Q102/Q122/Q088 probes remain correctly classified as ordinary
posting plans with zero relation target reads and zero edge visits. Their
schema manifests are byte-identical, but the repository schema compatibility
validator does not accept the benchmark adapter field in either extended run.
The standard 150-case comparison remains the authoritative schema-compatible
benchmark.

## Verification

```sh
python3 verify_relation_repair.py . \
  --archived ../assignment-3-relations-final-integration \
  --output relation-repair-comparison.json --self-test
```

The self-test accepts a valid control and rejects:

- a missing case on both sides;
- a missing repetition on both sides;
- a duplicate plan;
- empty evidence on both sides;
- the same failed query injected on both sides;
- a duplicate Q088 digest key on A2;
- a null Q088 digest on both sides;
- a mismatched C binary-build revision.

Archived digest files are parsed with duplicate-key detection. Their case set
must match exactly, every value must be 64 lowercase hexadecimal characters,
and only then are the two sides compared. Every archived side also checks both
the source and binary-build revisions from its run manifest.

## Q136

The focused rerun used three fresh B/final batches in alternating order, three
repetitions per batch, four query workers and zero verified-cache bytes.

| Batch | B median | Final median |
|---|---:|---:|
| 1 | 3.835981 ms | 3.855142 ms |
| 2 | 3.864366 ms | 3.936682 ms |
| 3 | 3.688415 ms | 3.972614 ms |
| All nine samples | 3.835981 ms | 3.936682 ms |

The original 0.411 ms / 13.6% signal therefore did not persist at that
magnitude; the focused difference is +0.101 ms / +2.63%. Ordered results and
all work counters still match.

The repeated per-commit sweep localizes the larger signal to
`0b36bc256f`, the split into `view`, `work` and `evaluation`:

| Revision | Median |
|---|---:|
| B | 3.925396 ms |
| A1 | 3.858380 ms |
| A2 | 3.930238 ms |
| A3 commit 1 | 3.918007 ms |
| A3 commit 2 | 4.409480 ms |
| Reviewed C | 4.442216 ms |
| Repair | 3.952558 ms |

Q136 executes the deep-offset sorted-posting path with `sort_strategy =
bounded_top_k` and records zero relation target reads, edge visits and bytes.
The 0.491 ms step at commit 2 is therefore a module-layout/code-generation
effect, not a changed relation traversal result. The ownership repair removes
0.490 ms of that step and returns the final median to within 0.027 ms
(+0.69%) of the repeated B median.

Reproduce the analysis with:

```sh
python3 analyze_q136.py q136 q136/analysis.json
```
