# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q069 | ascii-off | 16528.216 | 16785.500 | 17640.496 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 16126.432 | 17562.921 | 18925.099 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.618 s; snapshot write 0.430 s; parses 148486; resident 457 MiB; peak resident 549 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.332/1.342/1.378 s over 5 runs.
- "unicode": build 2.605 s; snapshot write 0.451 s; parses 148486; resident 707 MiB; peak resident 707 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.325/1.331/1.414 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
