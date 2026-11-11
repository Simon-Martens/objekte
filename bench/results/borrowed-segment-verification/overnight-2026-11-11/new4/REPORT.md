# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q069 | ascii-off | 13326.512 | 14036.075 | 14684.694 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 13487.765 | 14135.361 | 14866.144 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.585 s; snapshot write 0.438 s; parses 148486; resident 541 MiB; peak resident 568 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.330/1.334/1.429 s over 5 runs.
- "unicode": build 2.627 s; snapshot write 0.441 s; parses 148486; resident 747 MiB; peak resident 747 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.336/1.342/1.446 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
