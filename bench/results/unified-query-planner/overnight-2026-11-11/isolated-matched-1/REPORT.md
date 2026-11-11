# Musenalm query checkpoint

Code revision: `05c00a3cbfa2f218ed063681eccbd9769386aef5`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q116 | ascii-off | 8474.621 | 10859.071 | 34849.882 | 70428 | 70428 | 0 | 20/21 | 0/21 | ok |
| Q128 | ascii-off | 373.612 | 393.519 | 848.003 | 3257 | 3257 | 0 | 20/21 | 0/21 | ok |
| Q130 | ascii-off | 1291.457 | 1425.227 | 1653.587 | 3257 | 3257 | 0 | 21/21 | 0/21 | ok |
| Q116 | unicode-off | 9108.772 | 11329.003 | 39843.263 | 70428 | 70428 | 0 | 20/21 | 0/21 | ok |
| Q128 | unicode-off | 384.512 | 413.908 | 1020.677 | 3257 | 3257 | 0 | 20/21 | 0/21 | ok |
| Q130 | unicode-off | 2284.973 | 3866.895 | 7769.416 | 3257 | 3257 | 0 | 21/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.563 s; snapshot write 0.547 s; parses 148486; resident 467 MiB; peak resident 548 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.325/1.328/1.414 s over 5 runs.
- "unicode": build 2.588 s; snapshot write 0.550 s; parses 148486; resident 744 MiB; peak resident 744 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.324/1.326/1.475 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
