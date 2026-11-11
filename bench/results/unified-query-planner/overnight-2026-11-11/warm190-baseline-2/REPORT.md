# Musenalm query checkpoint

Code revision: `05c00a3cbfa2f218ed063681eccbd9769386aef5`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q190 | ascii-warm | 75380.387 | 85054.742 | 109807.615 | 3821 | 0 | 3821 | 21/21 | 21/21 | ok |
| Q190 | unicode-warm | 74422.587 | 87427.409 | 118520.694 | 3821 | 0 | 3821 | 21/21 | 21/21 | ok |

## Index construction

- "ascii": build 2.624 s; snapshot write 0.551 s; parses 148486; resident 469 MiB; peak resident 553 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.330/1.346/1.438 s over 5 runs.
- "unicode": build 2.607 s; snapshot write 0.557 s; parses 148486; resident 733 MiB; peak resident 733 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.342/1.355/1.563 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
