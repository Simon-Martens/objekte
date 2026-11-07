# Musenalm query checkpoint

Code revision: `20261604bebf949131441c8262f1f442bce4207c`. Repetitions: 31. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q113 | unicode-off | 2593.074 | 2744.942 | 3623.350 | 951 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q114 | unicode-off | 2790.267 | 4477.930 | 8902.620 | 3977 | 4928 | 4928 | 30/31 | 0/31 | ok |

## Index construction

- "unicode": build 11.990 s; snapshot write 0.303 s; parses 148486; resident 274 MiB; peak resident 371 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.502/0.510/0.540 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
