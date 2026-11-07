# Musenalm query checkpoint

Code revision: `8dd0c2aa6df3fa8c1f62b3dc97ead9e732bece97`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q031 | unicode-off | 137.542 | 137.542 | 137.542 | 203 | 203 | 0 | 0/1 | 0/1 | ok |

## Index construction

- "unicode": build 15.301 s; snapshot write 0.348 s; parses 148486; resident 264 MiB; peak resident 355 MiB; estimated index 77 MiB; snapshot 112 MiB; fresh-process restore min/median/max 0.541/0.558/0.565 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
