# Musenalm query checkpoint

Code revision: `6e6db5f62a4427a4a090ce078f37898f88572865`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q031 | unicode-off | 137.177 | 137.177 | 137.177 | 203 | 203 | 0 | 0/1 | 0/1 | ok |

## Index construction

- "unicode": build 15.225 s; snapshot write 0.291 s; parses 148486; resident 264 MiB; peak resident 354 MiB; estimated index 77 MiB; snapshot 112 MiB; fresh-process restore min/median/max 0.547/0.559/0.569 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
