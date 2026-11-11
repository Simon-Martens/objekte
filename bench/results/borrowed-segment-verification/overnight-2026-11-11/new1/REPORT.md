# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q069 | ascii-off | 16237.712 | 16434.912 | 19221.396 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 16517.046 | 18192.734 | 20239.058 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.569 s; snapshot write 0.432 s; parses 148486; resident 505 MiB; peak resident 537 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.339/1.351/1.387 s over 5 runs.
- "unicode": build 2.608 s; snapshot write 0.449 s; parses 148486; resident 682 MiB; peak resident 685 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.331/1.339/1.426 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
