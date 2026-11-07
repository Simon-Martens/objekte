# Musenalm query checkpoint

Code revision: `f374101b4839f28199a99677118ac96869203bb0`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 230430.938 | 274835.757 | 314950.453 | 65548 | 148486 | 148486 | 4/5 | 0/5 | ok |
| Q007 | ascii-off | 283946.152 | 378312.570 | 427213.546 | 147798 | 148486 | 148486 | 4/5 | 0/5 | ok |
| Q006 | unicode-off | 249408.500 | 281229.814 | 317887.126 | 65548 | 148486 | 148486 | 4/5 | 0/5 | ok |
| Q007 | unicode-off | 312384.107 | 366084.300 | 557538.109 | 147798 | 148486 | 148486 | 4/5 | 0/5 | ok |

## Index construction

- "ascii": build 5.582 s; snapshot write 0.385 s; parses 148486; resident 483 MiB; peak resident 574 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.527/0.539/0.546 s over 5 runs.
- "unicode": build 1.354 s; snapshot write 0.376 s; parses 148486; resident 751 MiB; peak resident 820 MiB; estimated index 68 MiB; snapshot 114 MiB; fresh-process restore min/median/max 0.526/0.543/0.561 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
