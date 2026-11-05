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

## Decisions

- Keep the generic search engine independent of Musenalm schema annotations.
- Inspect archived implementations for algorithms; adapt only components that
  preserve the new record-level semantics. No archived crate joins this workspace.
- Use the existing checkout's seven Musenalm schemas as fixture authority.

## Measurements

Pending runnable query and restore implementations. No timings are inferred
from the archived program.
