# Musenalm query checkpoint

Code revision: `45a81a85be5f8a122a3272ce3369ef2ed81632d8`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 12300.293 | 14354.911 | 88598.557 | 88598.557 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q027 | ascii-off | 1312.900 | 1475.343 | 1896.165 | 1896.165 | 215 | 578 | 578 | 0/3 | ok |
| Q043 | ascii-off | 5722.411 | 5770.456 | 6275.838 | 6275.838 | 3022 | 3022 | 0 | 0/3 | ok |
| Q119 | ascii-off | 4771.777 | 4792.943 | 5859.068 | 5859.068 | 133948 | 133948 | 0 | 0/3 | ok |
| Q001 | unicode-off | 12342.112 | 14630.332 | 86167.738 | 86167.738 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q027 | unicode-off | 1383.926 | 1413.011 | 1635.597 | 1635.597 | 215 | 578 | 578 | 0/3 | ok |
| Q043 | unicode-off | 5727.853 | 5733.120 | 6185.941 | 6185.941 | 3022 | 3022 | 0 | 0/3 | ok |
| Q119 | unicode-off | 4875.982 | 4886.926 | 5860.134 | 5860.134 | 133948 | 133948 | 0 | 0/3 | ok |

## Index construction

- "ascii": build 1.091 s; parses 148486; resident 841 MiB; peak resident 904 MiB; estimated index 0 MiB; rkyv archive 132 MiB.
- "unicode": build 1.071 s; parses 148486; resident 1088 MiB; peak resident 1107 MiB; estimated index 0 MiB; rkyv archive 132 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
