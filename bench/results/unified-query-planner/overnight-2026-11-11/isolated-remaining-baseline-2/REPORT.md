# Musenalm query checkpoint

Code revision: `05c00a3cbfa2f218ed063681eccbd9769386aef5`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q072 | ascii-off | 5755.834 | 6157.378 | 15958.541 | 1287 | 2251 | 2251 | 20/21 | 0/21 | ok |
| Q136 | ascii-off | 53421.653 | 81753.370 | 164992.620 | 133948 | 133948 | 0 | 20/21 | 0/21 | ok |
| Q072 | unicode-off | 5913.560 | 6424.420 | 16336.572 | 1287 | 2251 | 2251 | 20/21 | 0/21 | ok |
| Q136 | unicode-off | 53079.931 | 105070.145 | 180263.820 | 133948 | 133948 | 0 | 20/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.613 s; snapshot write 0.550 s; parses 148486; resident 472 MiB; peak resident 547 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.356/1.360/1.435 s over 5 runs.
- "unicode": build 2.612 s; snapshot write 0.563 s; parses 148486; resident 734 MiB; peak resident 734 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.337/1.341/1.559 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
