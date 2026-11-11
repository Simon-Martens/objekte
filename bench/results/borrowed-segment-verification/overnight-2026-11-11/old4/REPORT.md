# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q069 | ascii-off | 16063.112 | 16417.829 | 16726.178 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 16059.165 | 16744.161 | 18043.572 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.587 s; snapshot write 0.430 s; parses 148486; resident 554 MiB; peak resident 580 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.317/1.338/1.378 s over 5 runs.
- "unicode": build 2.624 s; snapshot write 0.452 s; parses 148486; resident 746 MiB; peak resident 746 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.325/1.342/1.423 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
