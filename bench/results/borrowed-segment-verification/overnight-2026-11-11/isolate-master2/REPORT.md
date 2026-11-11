# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q022 | ascii-off | 8538.239 | 8968.688 | 20363.343 | 1493 | 1504 | 1504 | 20/21 | 0/21 | ok |
| Q110 | ascii-off | 14095.489 | 16328.924 | 22468.137 | 4911 | 4928 | 4928 | 20/21 | 0/21 | ok |
| Q190 | ascii-off | 18627.763 | 21994.948 | 33122.060 | 3821 | 3821 | 3821 | 20/21 | 0/21 | ok |
| Q022 | unicode-warm | 6.833 | 7.955 | 34.545 | 1493 | 0 | 0 | 21/21 | 21/21 | ok |
| Q110 | unicode-warm | 5.851 | 6.282 | 21.711 | 4911 | 0 | 0 | 21/21 | 21/21 | ok |
| Q190 | unicode-warm | 76968.106 | 81288.752 | 87624.403 | 3821 | 0 | 3821 | 21/21 | 21/21 | ok |

## Index construction

- "ascii": build 2.581 s; snapshot write 0.433 s; parses 148486; resident 476 MiB; peak resident 547 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.329/1.342/1.384 s over 5 runs.
- "unicode": build 2.644 s; snapshot write 0.451 s; parses 148486; resident 748 MiB; peak resident 748 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.343/1.353/1.435 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
