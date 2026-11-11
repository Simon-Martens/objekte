# Musenalm query checkpoint

Code revision: `a1bb0990e86a8ed7a8c35c3e4a1610b5077c3bc3`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q022 | ascii-off | 8856.086 | 11108.919 | 19020.060 | 1493 | 1504 | 1504 | 20/21 | 0/21 | ok |
| Q110 | ascii-off | 14597.732 | 18096.875 | 23795.390 | 4911 | 4928 | 4928 | 20/21 | 0/21 | ok |
| Q190 | ascii-off | 20499.709 | 23662.340 | 35362.569 | 3821 | 3821 | 3821 | 20/21 | 0/21 | ok |
| Q022 | unicode-warm | 11.301 | 12.574 | 27.732 | 1493 | 0 | 0 | 21/21 | 21/21 | ok |
| Q110 | unicode-warm | 5.841 | 8.877 | 33.744 | 4911 | 0 | 0 | 21/21 | 21/21 | ok |
| Q190 | unicode-warm | 76511.719 | 80632.700 | 86737.628 | 3821 | 0 | 3821 | 21/21 | 21/21 | ok |

## Index construction

- "ascii": build 2.586 s; snapshot write 0.432 s; parses 148486; resident 546 MiB; peak resident 572 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.338/1.352/1.387 s over 5 runs.
- "unicode": build 2.587 s; snapshot write 0.443 s; parses 148486; resident 703 MiB; peak resident 703 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.342/1.354/1.428 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
