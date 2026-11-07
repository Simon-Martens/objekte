# Musenalm query checkpoint

Code revision: `45084cdbc3c7041b204c1771c5d18b9c7bb14092`. Repetitions: 20. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | ascii-off | 6923.742 | 7535.301 | 9536.707 | 133948 | 133948 | 0 | 19/20 | 0/20 | ok |

## Index construction

- "ascii": build 1.207 s; snapshot write 0.283 s; parses 148486; resident 533 MiB; peak resident 624 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.529/0.532/0.546 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
