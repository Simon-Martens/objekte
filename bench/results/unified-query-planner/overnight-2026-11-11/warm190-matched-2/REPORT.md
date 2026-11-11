# Musenalm query checkpoint

Code revision: `05c00a3cbfa2f218ed063681eccbd9769386aef5`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q190 | ascii-warm | 75977.367 | 88017.117 | 121342.505 | 3821 | 0 | 3821 | 21/21 | 21/21 | ok |
| Q190 | unicode-warm | 74740.694 | 88718.154 | 108874.261 | 3821 | 0 | 3821 | 21/21 | 21/21 | ok |

## Index construction

- "ascii": build 2.592 s; snapshot write 0.542 s; parses 148486; resident 534 MiB; peak resident 559 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.341/1.352/1.433 s over 5 runs.
- "unicode": build 2.607 s; snapshot write 0.561 s; parses 148486; resident 732 MiB; peak resident 732 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.347/1.350/1.511 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
