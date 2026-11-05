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

## Measurements

Pending runnable query and restore implementations. No timings are inferred
from the archived program.

The manifest is a transcription of Section 21 of `implement.md`; fixture
schema and query-semantic validation remain pending until registration and
binding exist. The report format reserves separate status and optional timing
fields so unsupported or failed cases cannot be recorded as successful zero
duration samples.
