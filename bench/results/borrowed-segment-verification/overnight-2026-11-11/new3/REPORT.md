# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q069 | ascii-off | 13182.613 | 13527.169 | 14129.781 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 13319.740 | 14339.735 | 15243.934 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.557 s; snapshot write 0.436 s; parses 148486; resident 536 MiB; peak resident 563 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.321/1.333/1.386 s over 5 runs.
- "unicode": build 2.608 s; snapshot write 0.449 s; parses 148486; resident 735 MiB; peak resident 735 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.342/1.347/1.412 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
