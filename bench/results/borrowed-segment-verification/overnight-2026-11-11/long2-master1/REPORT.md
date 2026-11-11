# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 100. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q049 | ascii-off | 307.217 | 316.555 | 1014.225 | 4702 | 4702 | 0 | 99/100 | 0/100 | ok |
| Q158 | ascii-off | 14495.999 | 16744.963 | 26098.943 | 3106 | 4928 | 4928 | 99/100 | 0/100 | ok |

## Index construction

- "ascii": build 2.544 s; snapshot write 0.439 s; parses 148486; resident 522 MiB; peak resident 548 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.330/1.344/1.446 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
