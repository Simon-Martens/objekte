# Musenalm query checkpoint

Code revision: `8dd0c2aa6df3fa8c1f62b3dc97ead9e732bece97`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q031 | unicode-off | 125.379 | 125.379 | 125.379 | 203 | 203 | 0 | 0/1 | 0/1 | ok |

## Index construction

- "unicode": build 15.234 s; snapshot write 0.349 s; parses 148486; resident 264 MiB; peak resident 355 MiB; estimated index 77 MiB; snapshot 112 MiB; fresh-process restore min/median/max 0.544/0.556/0.568 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
