# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q069 | ascii-off | 15948.649 | 16217.664 | 17797.862 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 15662.761 | 15941.605 | 17512.075 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.558 s; snapshot write 0.447 s; parses 148486; resident 465 MiB; peak resident 550 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.336/1.350/1.427 s over 5 runs.
- "unicode": build 2.592 s; snapshot write 0.455 s; parses 148486; resident 726 MiB; peak resident 726 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.338/1.344/1.441 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
