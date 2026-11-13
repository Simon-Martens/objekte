# Musenalm query checkpoint

Code revision: `7a9dee532e75bf167b6e4eabfd52277c79bb4df1`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q141 | ascii-off | 5038.607 | 5673.583 | 7955.560 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 4534.579 | 5090.607 | 11412.610 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q209 | ascii-off | 1653.410 | 1919.765 | 3346.047 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q214 | ascii-off | 3181.282 | 3830.516 | 4722.687 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q141 | unicode-off | 4139.139 | 4390.837 | 6273.477 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 4734.713 | 4959.688 | 7867.648 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q209 | unicode-off | 1777.701 | 1884.485 | 2628.597 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q214 | unicode-off | 2282.685 | 3262.955 | 3835.528 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 1.039 s; snapshot write 0.271 s; parses 148486; resident 487 MiB; peak resident 509 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.616/0.618/0.686 s over 5 runs.
- "unicode": build 1.046 s; snapshot write 0.256 s; parses 148486; resident 665 MiB; peak resident 665 MiB; estimated index 69 MiB; snapshot 126 MiB; fresh-process restore min/median/max 0.612/0.639/0.702 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
