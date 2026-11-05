# Search core progress

The new workspace builds only `core/`. The archived `old/` implementation and
root `examples/` are preserved. The earlier code is available for algorithms
and behavior checks, especially its bigram storage, typed filtering, and ICU
sorting. The new crate will use record slots and file-to-record ownership;
the archived index's file slots cannot be reused directly.

No applicable new-core baseline exists. The archived benchmark data uses a
different implementation and query contract. The first correct new-core
query/build and restore revisions will establish their respective baselines.

## Steps

| Step | Status | Validation | Notes |
|---|---|---|---|
| C01 | complete | `cargo test --workspace` | New workspace; fixture path check |
| C02 | complete | `cargo fmt --all && cargo test --workspace` | 150 fixed cases, four profile definitions, report envelopes |
| C03 | complete | `cargo fmt --all && cargo test --workspace` | Typed values, precise numeric ordering, temporal types, paths, IDs |
| C04 | complete | `cargo fmt --all && cargo test --workspace` | File inventory, checked ranges, both record mappings |
| C05 | complete | `cargo fmt --all && cargo test --workspace` | JSON/JSONL framing, standalone range parsing, fused extraction callback |
| C06 | complete | `cargo fmt --all && cargo test --workspace` | Separate FTS input, FILTER parser, all fixed filters parse |
| C07 | complete | `cargo fmt --all && cargo test --workspace` | Typed binding and independent scan evaluation |
| C08 | complete | `cargo test --workspace --all-targets`; CLI Q031, Q070 | Fixture adapter, JSON/filename registration, reference CLI |
| C09 | complete | `cargo test --workspace --all-targets`; CLI Q031 | Fused typed B-tree postings, presence/null/array length, coverage |
| C10 | complete | `cargo test --workspace --all-targets`; CLI Q031 | Exact posting/metadata sets, restricted scan, zero-read exact matches |
| C11 | complete | `cargo test --workspace --all-targets`; CLI places FTS Leipzig | ASCII normal/skip bigrams on record text segments |
| C12 | complete | `cargo test --workspace --all-targets`; Q013 ASCII/Unicode | Repository alphabet setting, observed Unicode pairs, equal 57 matches |
| C13 | complete | `cargo test --workspace --all-targets`; Q070 indexed/reference | Folded custom extraction, bigrams, scanner, selected FTS |
| C14 | complete | `cargo test --workspace --all-targets`; Q126/Q127 indexed/reference | ICU/typed sort columns, stable ties, multi-field order |
| C15 | complete | `cargo test --workspace --all-targets`; Q031 logical page | COUNT, bounded page winners, three payload modes, grouped range reads |
| C16a | complete | release smoke, all 150 ASCII cases (1 repetition) | Persistent-process timing runner and report files |
| C16b | complete | 900 raw samples, 0 failures | Baseline from `472c00d838` for 150 cases × ASCII/Unicode off × 3 repetitions |
| C17 | complete | `cargo test --workspace --all-targets` | Maintained posting/source stats and per-term costed path enumeration |
| C18 | complete | `cargo test --workspace --all-targets`; rare-text/permutation fixture | Flattened AND, costed AND/OR order, actual-cardinality adaptation |
| C19 | complete | `cargo test --workspace --all-targets`; Q031/Q070/Q141 uncached subset | Borrowed restricted postings, EXPLAIN/ANALYZE, per-case trace output |
| C20 | complete | `cargo test --workspace --all-targets` | 4 MiB bounded repository-local prepared bindings and separate hit counter |
| C21 | complete | `cargo test --workspace --all-targets`; four-profile Q031/Q070/Q141 subset | Bounded complete ordered membership/evidence cache, page payload reads |

## Decisions

- Keep the generic search engine independent of Musenalm schema annotations.
- Inspect archived implementations for algorithms; adapt only components that
  preserve the new record-level semantics. No archived crate joins this workspace.
- Use the existing checkout's seven Musenalm schemas as fixture authority.
- Normalize finite integral floats into exact integers when representable;
  otherwise compare them against integers without rounding the integer to
  float. A date, datetime, and Unix seconds remain distinct variants.
- A collection generation owns compact record slots. Its file-to-record
  offset table and reverse records are validated together; file identity is
  repository-wide and a file can participate in multiple collections.
- Source readers frame original byte ranges. The extraction callback receives
  one parsed logical record plus separate field text segments for every range.
- The reference evaluator resolves all branches before execution and applies
  Unicode case folding through `unicode-casefold` 0.2. It is kept independent
  of optimized access paths for later comparisons.
- The Musenalm adapter reads each checkout schema, all nested fields and
  `x-index-paths`, and the declared JSON/filename policy. A targeted fixture
  test preserves `contents.numbering = 32.01` as a number.
- Lookup postings are built during the same extraction callback that creates
  record mappings. Explicit schema `index` and `x-index-paths` determine the
  posted fields; presence/null/cardinality are attached to those fields.
- The initial executor intersects available exact terms by resulting
  cardinality, then scans only survivors for unresolved terms. Metadata
  predicates run from inventory without source reads. More detailed path
  costs and adaptive choices follow in C17–C19.
- The ASCII raw filter stores observed normal and skip pairs by record slot.
  It lowercases only pair keys for conservative filtering, then verifies the
  original case-sensitive text. Unsupported or short literals scan.
- Unicode mode stores only observed character pairs and uses separate normal
  and skip postings. Alphabet choice changes candidate reduction only. The
  archived bigram implementation was inspected for semantics, but this new
  record-slot implementation does not copy its storage code.
- `agents.name:folded` has a separate versioned transform registration and
  folded bigram path. Q070 matched one record in both optimized and reference
  modes; indexed verification parsed one record versus 4,010 in the reference.
- String sort keys use pinned ICU4X 2.4.0 compiled data with locale `de` and
  default options; numeric/temporal keys keep their logical types. Null and
  missing values remain last in both directions, followed by an identity tie.
- The optimized executor keeps at most `offset + limit` page winners. A
  verification parse can be moved into a winning logical payload; index-only
  membership reads only requested page bodies. Candidate ranges are grouped
  by file and nearby windows share one source read.
- Each term now enumerates compatible metadata, posting, typed sort-column,
  dictionary, raw/custom bigram, and scanner paths. Costs use coverage,
  cardinalities, source bytes and expected residual work; estimates affect
  path choice only.
- Boolean planning now flattens associative AND groups and greedily chooses
  each next operation against the actual surviving count. A selective text
  bigram can run before a broad posting, and a strong FILTER restriction can
  bypass global FTS filtering. OR branches are cost ordered but retain their
  full eligible domains when another branch has no index.
- Roaring uses sorted array containers for sparse sets and bitmaps for dense
  containers; direct restricted equality/presence lookups intersect borrowed
  postings without cloning a broad source set. EXPLAIN is read-only over
  resident indexes; EXPLAIN ANALYZE records term order, alternatives, estimates,
  source work, and proof counts. The benchmark saves traces separately from
  primary timing samples.
- The prepared cache stores typed, validated collection-specific bindings under
  a configurable 4 MiB default budget. Its lifetime is one immutable
  repository generation; reopening creates a new cache. Pagination, return
  mode, and evidence can share a prepared binding. Hits are reported separately
  from verified-result reuse.
- The verified cache retains complete ordered member lists and requested
  evidence under an independent byte budget, including empty lists. It never
  stores record bodies. Repeated pages and return modes share membership;
  requested bytes/logical payloads still use revision-checked range reads.
  The benchmark can switch between disabled and 64 MiB warm profiles on one
  resident repository without rebuilding indexes.

## Measurements

The initial release smoke built 148,486 Musenalm records once in 13.8 s with
ASCII bigrams, then executed all 150 queries with one measured repetition and
zero failures. The saved baseline is
[`bench/results/musenalm/baseline-472c00d838/REPORT.md`](../bench/results/musenalm/baseline-472c00d838/REPORT.md)
with 900 raw samples, all 150 cases under both cache-disabled alphabets, and
zero failures. On this revision the full ASCII build took 13.695 s and the
Unicode build 15.039 s. Restore and verified-cache measurements remain
pending. No timings are inferred from the archived program.

A three-case uncached check after C19 (three repetitions; ASCII and Unicode)
matched the baseline results. For Q141 the ASCII median was 25.955 ms versus
28.233 ms at the baseline, with 982 candidates in both runs; Unicode was
22.575 ms versus 27.905 ms, with 842 baseline and 982 current candidates.
Q031 (roughly 50 µs) and Q070 (roughly 0.8 ms) differed by tens of
microseconds on this short sample, so they do not establish a speed trend.
The check is under `/tmp/objekte-c19-check` and is distinct from the final
four-profile run.

The C21 three-case four-profile check produced 36 successful samples. Every
warm sample reported a verified-result hit; no disabled sample did. ASCII
medians for Q031/Q070/Q141 were 59.777 µs / 815.909 µs / 25.068 ms without
the verified cache and 6.264 µs / 2.197 µs / 10.400 µs warm. This subset is
saved at `/tmp/objekte-c21-check`; the full final workload remains required.

The manifest is a transcription of Section 21 of `implement.md`; all 150
queries parse, bind, and execute against the current fixture. The report format
reserves separate status and optional timing
fields so unsupported or failed cases cannot be recorded as successful zero
duration samples.
