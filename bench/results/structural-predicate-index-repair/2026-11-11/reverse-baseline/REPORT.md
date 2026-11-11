# Musenalm query checkpoint

Code revision: `95f1736d448a01d94650afd237149c158c0e7a8a`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | ascii-off | 25283.344 | 26081.382 | 46063.642 | 133948 | 133948 | 0 | 10/11 | 0/11 | ok |
| Q141 | ascii-off | 7268.719 | 8357.452 | 12091.244 | 823 | 982 | 982 | 10/11 | 0/11 | ok |
| Q190 | ascii-off | 19769.029 | 25699.886 | 40082.682 | 3821 | 3821 | 3821 | 10/11 | 0/11 | ok |
| Q119 | unicode-off | 24972.700 | 25998.427 | 38360.478 | 133948 | 133948 | 0 | 10/11 | 0/11 | ok |
| Q141 | unicode-off | 7147.521 | 8294.564 | 15518.298 | 823 | 982 | 982 | 10/11 | 0/11 | ok |
| Q190 | unicode-off | 19121.865 | 21629.063 | 33328.440 | 3821 | 3821 | 3821 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 2.489 s; snapshot write 0.435 s; parses 148486; resident 520 MiB; peak resident 546 MiB; estimated index 65 MiB; snapshot 121 MiB; fresh-process restore min/median/max 1.327/1.334/1.408 s over 5 runs.
- "unicode": build 2.536 s; snapshot write 0.443 s; parses 148486; resident 667 MiB; peak resident 667 MiB; estimated index 68 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.338/1.343/1.436 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
