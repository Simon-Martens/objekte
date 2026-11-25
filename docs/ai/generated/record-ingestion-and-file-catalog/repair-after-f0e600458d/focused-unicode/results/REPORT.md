# Musenalm query checkpoint

Code revision: `f0e600458d8804b57efb0a8a2b91478d1a39f667`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 36561.077 | 38458.349 | 41399.796 | 41399.796 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | unicode-off | 8325.196 | 9381.022 | 10225.402 | 10225.402 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | unicode-off | 3.740 | 3.918 | 30.390 | 30.390 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | unicode-off | 20.161 | 21.708 | 43.746 | 43.746 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | unicode-off | 1518.933 | 1528.193 | 1727.234 | 1727.234 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | unicode-off | 3270.905 | 3293.061 | 3628.664 | 3628.664 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | unicode-off | 192.887 | 195.636 | 244.093 | 244.093 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | unicode-off | 2637.278 | 2789.040 | 4739.000 | 4739.000 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | unicode-off | 63113.422 | 63919.540 | 65368.438 | 65368.438 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "unicode": build 0.697 s; parses 148486; resident 596 MiB; peak resident 637 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
