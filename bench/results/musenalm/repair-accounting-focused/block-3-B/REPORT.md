# Musenalm query checkpoint

Code revision: `c5787c1d8099a2ac99fe9157035451d41ccd4015`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 201241.771 | 265828.582 | 295507.497 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q025 | ascii-off | 94601.202 | 126949.739 | 165581.483 | 60319 | 67748 | 67748 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 2026.243 | 4585.759 | 6539.552 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q116 | ascii-off | 109123.370 | 145639.800 | 177400.034 | 70428 | 133948 | 133948 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.359 s; snapshot write 0.270 s; parses 148486; resident 251 MiB; peak resident 339 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.607/0.618/0.657 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
