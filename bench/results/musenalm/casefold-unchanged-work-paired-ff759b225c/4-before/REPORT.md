# Musenalm query checkpoint

Code revision: `20261604bebf949131441c8262f1f442bce4207c`. Repetitions: 31. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q113 | unicode-off | 2616.126 | 2788.910 | 4399.956 | 951 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q114 | unicode-off | 2879.141 | 4176.266 | 11090.253 | 3977 | 4928 | 4928 | 30/31 | 0/31 | ok |

## Index construction

- "unicode": build 12.107 s; snapshot write 0.299 s; parses 148486; resident 274 MiB; peak resident 371 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.507/0.517/0.562 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
