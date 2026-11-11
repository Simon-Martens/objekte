# Musenalm query checkpoint

Code revision: `a1bb0990e86a8ed7a8c35c3e4a1610b5077c3bc3`. Repetitions: 100. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q049 | ascii-off | 303.800 | 311.054 | 1010.037 | 4702 | 4702 | 0 | 99/100 | 0/100 | ok |
| Q158 | ascii-off | 14928.961 | 17402.709 | 27179.914 | 3106 | 4928 | 4928 | 99/100 | 0/100 | ok |

## Index construction

- "ascii": build 2.610 s; snapshot write 0.435 s; parses 148486; resident 466 MiB; peak resident 547 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.343/1.346/1.404 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
