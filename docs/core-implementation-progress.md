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

## Measurements

Pending runnable query and restore implementations. No timings are inferred
from the archived program.

The manifest is a transcription of Section 21 of `implement.md`; fixture
schema and query-semantic validation remain pending until registration and
binding exist. The report format reserves separate status and optional timing
fields so unsupported or failed cases cannot be recorded as successful zero
duration samples.
