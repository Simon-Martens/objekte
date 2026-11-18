# Musenalm query checkpoint

Code revision: `71636c4fe3343b0b1dea67c0ade31e96b68fca4b`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 3010.387 | 4642.612 | 6027.605 | 6027.605 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 4593.919 | 4833.526 | 5011.592 | 5011.592 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 2379.794 | 2437.859 | 2703.582 | 2703.582 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 19.299 | 19.924 | 26.445 | 26.445 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 18.111 | 18.589 | 25.087 | 25.087 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 16.091 | 16.765 | 19.575 | 19.575 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.835 s; parses 148486; resident 685 MiB; peak resident 726 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
