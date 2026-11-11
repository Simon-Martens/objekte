# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q069 | ascii-off | 16134.588 | 16424.422 | 16876.952 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 15698.298 | 16458.557 | 16895.347 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.538 s; snapshot write 0.437 s; parses 148486; resident 445 MiB; peak resident 528 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.342/1.349/1.389 s over 5 runs.
- "unicode": build 2.619 s; snapshot write 0.441 s; parses 148486; resident 651 MiB; peak resident 651 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.331/1.367/1.381 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
