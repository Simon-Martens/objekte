# Query source counter semantics

- Category: bug
- Title: `distinct_files` and `bytes_read` do not describe physical query I/O
- Originating task: `core-refactoring-completion-source-preparation-repair`
- Observed at: A1 `b4172a1749750ee1c87ef74a56f5ee7cec6c40ee` and repair source
  `d07b700a7188efd6b45ec542f197b78a211cb88a`
- Code locations: `crates/core/src/measurement/mod.rs`,
  `crates/core/src/query_execution/runner/mod.rs`,
  `crates/core/src/source/access/session/mod.rs`

## Observed behavior

On the four-file JSON fixture, public `SearchResult::counters` and a narrowly
filtered `strace` of successful `openat`/`read` operations on the fixture paths
disagree:

| Query | Public opens | Trace opens | Public distinct | Trace distinct | Public bytes | Trace bytes |
|---|---:|---:|---:|---:|---:|---:|
| `exact_page` | 1 | 1 | 1 | 1 | 0 | 60 |
| `combined_old` | 3 | 6 | 6 | 3 | 165 | 330 |
| `combined_number` | 3 | 5 | 5 | 3 | 167 | 286 |

The combined queries report selected payload work separately as 3 reads/165
bytes and 2 reads/119 bytes. Adding those selected-payload bytes to
`bytes_read` explains the trace totals, but `bytes_read` itself remains
incomplete as a physical-read counter. `distinct_files` can exceed the number
of fixture files and the number of successful opens because selected-file set
sizes are added to an existing distinct count.

## Expected behavior

`files_opened` should count successful physical opens, `distinct_files` should
count the union of successful open paths, and `bytes_read` should account for
all successful source-byte reads requested by the query. If selected-payload
work is intentionally separate, the public documentation should make that
separation explicit and no counter should claim an impossible distinct-file
count.

## Impact

The counters remain useful for A1/candidate parity checks but are unsafe as
direct measurements of unique files opened or total source bytes requested.
This behavior predates the Assignment 2 preparation moves and is outside its
ownership refactor.

## Reproduction

Use the Assignment 2 source-read fixture and
`docs/ai/generated/core-refactoring-completion/assignment-2-source-preparation/read_probe.rs`.
Run it under:

```sh
strace -f -ttt -yy \
  -e trace=openat,read,pread64,lseek,close \
  -o trace.log cargo run --quiet --locked -p objekte-search-core \
  --example assignment_2_read_probe -- \
  --root "$fixture_root" --expected-sha "$tested_sha" \
  --trace-dir "$fixture_root/markers"
```

Then run `trace_io.py` with the trace, marker directory and fixture root. The
recorded results are `a1-reads.json`, `candidate-reads.json`, `a1-trace.json`
and `candidate-trace.json` in the same evidence directory.

## Possible approaches

- Evidence-based: treat physical trace accounting as authoritative and correct
  the public counter aggregation in separate maintenance work.
- Evidence-based: if selected payload reads are intended to remain separate,
  rename or document the fields so `distinct_files` and `bytes_read` are not
  interpreted as complete physical I/O.
- Hypothesis: adding set sizes and counts at different aggregation layers may
  be unintentional; a single per-query source-access ledger would make the
  semantics explicit.
