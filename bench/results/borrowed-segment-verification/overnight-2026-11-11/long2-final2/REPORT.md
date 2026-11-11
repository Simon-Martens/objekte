# Musenalm query checkpoint

Code revision: `a1bb0990e86a8ed7a8c35c3e4a1610b5077c3bc3`. Repetitions: 100. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q049 | ascii-off | 306.796 | 316.345 | 1043.981 | 4702 | 4702 | 0 | 99/100 | 0/100 | ok |
| Q158 | ascii-off | 14574.456 | 16791.701 | 27122.135 | 3106 | 4928 | 4928 | 99/100 | 0/100 | ok |

## Index construction

- "ascii": build 2.621 s; snapshot write 0.431 s; parses 148486; resident 526 MiB; peak resident 552 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.345/1.355/1.393 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
