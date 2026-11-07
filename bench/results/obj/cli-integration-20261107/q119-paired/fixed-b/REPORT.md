# Musenalm query checkpoint

Code revision: `45084cdbc3c7041b204c1771c5d18b9c7bb14092`. Repetitions: 20. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | ascii-off | 6895.934 | 7563.173 | 9102.425 | 133948 | 133948 | 0 | 19/20 | 0/20 | ok |

## Index construction

- "ascii": build 1.215 s; snapshot write 0.251 s; parses 148486; resident 520 MiB; peak resident 611 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.526/0.535/0.551 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
