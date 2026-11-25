# Musenalm query checkpoint

Code revision: `732725bcfba34e11f6b85b0125e0e447151fb03a`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 35682.952 | 36378.575 | 42450.084 | 42450.084 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | unicode-off | 8197.419 | 8262.062 | 9615.693 | 9615.693 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | unicode-off | 3.650 | 3.938 | 30.358 | 30.358 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | unicode-off | 21.587 | 22.606 | 45.490 | 45.490 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | unicode-off | 1517.778 | 1535.407 | 1698.145 | 1698.145 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | unicode-off | 3173.991 | 3198.379 | 3472.023 | 3472.023 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | unicode-off | 194.756 | 195.663 | 245.549 | 245.549 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | unicode-off | 2683.279 | 2817.755 | 4721.778 | 4721.778 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | unicode-off | 62525.978 | 64120.739 | 64901.171 | 64901.171 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "unicode": build 0.779 s; parses 148486; resident 607 MiB; peak resident 647 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
