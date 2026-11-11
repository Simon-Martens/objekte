# Musenalm query checkpoint

Code revision: `00f501902c8aee2a66da5cb829b56ab89dbf0ac8`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q190 | ascii-off | 19386.662 | 23404.039 | 61287.270 | 3821 | 3821 | 3821 | 0/21 | 0/21 | ok |
| Q190 | unicode-off | 18600.314 | 23025.698 | 55538.900 | 3821 | 3821 | 3821 | 0/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.572 s; snapshot write 0.554 s; parses 148486; resident 506 MiB; peak resident 546 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.335/1.350/1.483 s over 5 runs.
- "unicode": build 2.590 s; snapshot write 0.587 s; parses 148486; resident 695 MiB; peak resident 695 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.342/1.367/1.556 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
