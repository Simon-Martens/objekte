# Musenalm query checkpoint

Code revision: `6e1e96d9ead1c32f30f2a4f1d42afbe100f9edb4`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 200367.804 | 232486.169 | 240547.647 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q025 | ascii-off | 84835.691 | 106691.906 | 129883.632 | 60319 | 67748 | 67748 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 2185.452 | 3432.207 | 6196.794 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q116 | ascii-off | 98033.691 | 117603.608 | 132588.171 | 70428 | 133948 | 133948 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.345 s; snapshot write 0.516 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.598/0.617/0.666 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
