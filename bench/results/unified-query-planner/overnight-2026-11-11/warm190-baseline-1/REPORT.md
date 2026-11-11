# Musenalm query checkpoint

Code revision: `05c00a3cbfa2f218ed063681eccbd9769386aef5`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q190 | ascii-warm | 76054.062 | 90030.810 | 106800.986 | 3821 | 0 | 3821 | 21/21 | 21/21 | ok |
| Q190 | unicode-warm | 75737.087 | 87265.344 | 109453.540 | 3821 | 0 | 3821 | 21/21 | 21/21 | ok |

## Index construction

- "ascii": build 2.556 s; snapshot write 0.548 s; parses 148486; resident 471 MiB; peak resident 538 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.333/1.336/1.430 s over 5 runs.
- "unicode": build 2.677 s; snapshot write 0.554 s; parses 148486; resident 744 MiB; peak resident 744 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.353/1.358/1.530 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
