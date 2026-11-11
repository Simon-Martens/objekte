# Musenalm query checkpoint

Code revision: `05c00a3cbfa2f218ed063681eccbd9769386aef5`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q116 | ascii-off | 9053.458 | 11270.173 | 35362.916 | 70428 | 70428 | 0 | 20/21 | 0/21 | ok |
| Q128 | ascii-off | 380.174 | 388.289 | 931.751 | 3257 | 3257 | 0 | 20/21 | 0/21 | ok |
| Q130 | ascii-off | 1272.280 | 1577.203 | 5565.075 | 3257 | 3257 | 0 | 21/21 | 0/21 | ok |
| Q116 | unicode-off | 9400.310 | 11018.780 | 37579.460 | 70428 | 70428 | 0 | 20/21 | 0/21 | ok |
| Q128 | unicode-off | 386.977 | 401.855 | 875.485 | 3257 | 3257 | 0 | 20/21 | 0/21 | ok |
| Q130 | unicode-off | 1306.465 | 4359.951 | 5842.888 | 3257 | 3257 | 0 | 21/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.560 s; snapshot write 0.546 s; parses 148486; resident 538 MiB; peak resident 564 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.324/1.331/1.416 s over 5 runs.
- "unicode": build 2.558 s; snapshot write 0.546 s; parses 148486; resident 670 MiB; peak resident 670 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.328/1.343/1.506 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
