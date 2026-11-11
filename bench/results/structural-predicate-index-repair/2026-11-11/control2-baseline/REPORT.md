# Musenalm query checkpoint

Code revision: `90b52907c7f9c9364305e38e1b689c9766cc84d0`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q048 | ascii-off | 214.554 | 217.049 | 687.194 | 2377 | 2377 | 0 | 6/7 | 0/7 | ok |
| Q049 | ascii-off | 309.273 | 311.397 | 597.976 | 4702 | 4702 | 0 | 6/7 | 0/7 | ok |
| Q071 | ascii-off | 4400.958 | 4662.190 | 6447.843 | 1003 | 1627 | 1627 | 6/7 | 0/7 | ok |
| Q076 | ascii-off | 31680.997 | 34973.918 | 46327.381 | 2206 | 12554 | 12554 | 6/7 | 0/7 | ok |
| Q135 | ascii-off | 11812.477 | 13971.373 | 24443.065 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q048 | unicode-off | 214.394 | 224.092 | 715.207 | 2377 | 2377 | 0 | 6/7 | 0/7 | ok |
| Q049 | unicode-off | 309.062 | 313.540 | 598.797 | 4702 | 4702 | 0 | 6/7 | 0/7 | ok |
| Q071 | unicode-off | 4299.407 | 4504.193 | 5875.545 | 1003 | 1627 | 1627 | 6/7 | 0/7 | ok |
| Q076 | unicode-off | 31259.403 | 32511.721 | 50051.585 | 2206 | 12554 | 12554 | 6/7 | 0/7 | ok |
| Q135 | unicode-off | 11879.754 | 17132.306 | 25623.528 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.479 s; snapshot write 0.437 s; parses 148486; resident 461 MiB; peak resident 542 MiB; estimated index 65 MiB; snapshot 121 MiB; fresh-process restore min/median/max 1.323/1.330/1.372 s over 5 runs.
- "unicode": build 2.510 s; snapshot write 0.437 s; parses 148486; resident 722 MiB; peak resident 722 MiB; estimated index 68 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.332/1.341/1.414 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
