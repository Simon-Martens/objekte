# Musenalm query checkpoint

Code revision: `4a93382f0404ae4cc6d1e7c028b7a48cacd006f4`. Repetitions: 20. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | ascii-off | 6751.634 | 7055.968 | 9776.861 | 133948 | 133948 | 0 | 19/20 | 0/20 | ok |

## Index construction

- "ascii": build 1.169 s; snapshot write 0.264 s; parses 148486; resident 517 MiB; peak resident 608 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.514/0.520/0.535 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
