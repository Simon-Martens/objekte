# Musenalm query checkpoint

Code revision: `a1bb0990e86a8ed7a8c35c3e4a1610b5077c3bc3`. Repetitions: 100. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q022 | ascii-off | 8558.858 | 10317.341 | 18526.242 | 1493 | 1504 | 1504 | 99/100 | 0/100 | ok |
| Q110 | ascii-off | 14383.520 | 16755.345 | 25286.381 | 4911 | 4928 | 4928 | 99/100 | 0/100 | ok |

## Index construction

- "ascii": build 2.592 s; snapshot write 0.436 s; parses 148486; resident 536 MiB; peak resident 562 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.334/1.340/1.437 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
