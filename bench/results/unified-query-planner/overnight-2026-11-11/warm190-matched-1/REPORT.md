# Musenalm query checkpoint

Code revision: `05c00a3cbfa2f218ed063681eccbd9769386aef5`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q190 | ascii-warm | 77027.041 | 90682.664 | 114978.379 | 3821 | 0 | 3821 | 21/21 | 21/21 | ok |
| Q190 | unicode-warm | 75987.868 | 89548.544 | 107031.870 | 3821 | 0 | 3821 | 21/21 | 21/21 | ok |

## Index construction

- "ascii": build 2.586 s; snapshot write 0.554 s; parses 148486; resident 541 MiB; peak resident 567 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.334/1.343/1.429 s over 5 runs.
- "unicode": build 2.602 s; snapshot write 0.568 s; parses 148486; resident 738 MiB; peak resident 738 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.335/1.342/1.563 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
