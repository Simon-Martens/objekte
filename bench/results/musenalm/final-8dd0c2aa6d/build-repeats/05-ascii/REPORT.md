# Musenalm query checkpoint

Code revision: `8dd0c2aa6df3fa8c1f62b3dc97ead9e732bece97`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q031 | ascii-off | 131.201 | 131.201 | 131.201 | 203 | 203 | 0 | 0/1 | 0/1 | ok |

## Index construction

- "ascii": build 14.225 s; snapshot write 0.314 s; parses 148486; resident 241 MiB; peak resident 324 MiB; estimated index 65 MiB; snapshot 101 MiB; fresh-process restore min/median/max 0.520/0.534/0.543 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
