# Musenalm query checkpoint

Code revision: `45084cdbc3c7041b204c1771c5d18b9c7bb14092`. Repetitions: 20. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | unicode-off | 6767.492 | 7493.239 | 9460.735 | 133948 | 133948 | 0 | 19/20 | 0/20 | ok |

## Index construction

- "unicode": build 1.266 s; snapshot write 0.255 s; parses 148486; resident 562 MiB; peak resident 654 MiB; estimated index 68 MiB; snapshot 114 MiB; fresh-process restore min/median/max 0.514/0.525/0.527 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
