# Musenalm query checkpoint

Code revision: `4a93382f0404ae4cc6d1e7c028b7a48cacd006f4`. Repetitions: 20. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | ascii-off | 6344.046 | 7173.719 | 9274.196 | 133948 | 133948 | 0 | 19/20 | 0/20 | ok |

## Index construction

- "ascii": build 1.147 s; snapshot write 0.293 s; parses 148486; resident 510 MiB; peak resident 601 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.511/0.519/0.527 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
