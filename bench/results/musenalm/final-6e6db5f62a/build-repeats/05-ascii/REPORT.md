# Musenalm query checkpoint

Code revision: `6e6db5f62a4427a4a090ce078f37898f88572865`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q031 | ascii-off | 139.830 | 139.830 | 139.830 | 203 | 203 | 0 | 0/1 | 0/1 | ok |

## Index construction

- "ascii": build 14.072 s; snapshot write 0.260 s; parses 148486; resident 241 MiB; peak resident 324 MiB; estimated index 65 MiB; snapshot 101 MiB; fresh-process restore min/median/max 0.529/0.542/0.558 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
