# Musenalm query checkpoint

Code revision: `7ce3f6e525fee77f46c71ad7c02a94c7d06a21b7`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 35717.070 | 37580.916 | 41127.147 | 41127.147 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | ascii-off | 8174.214 | 8190.688 | 9587.712 | 9587.712 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | ascii-off | 3.776 | 3.860 | 26.248 | 26.248 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | ascii-off | 20.993 | 21.820 | 43.149 | 43.149 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | ascii-off | 1522.479 | 1530.357 | 1676.886 | 1676.886 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | ascii-off | 3179.272 | 3197.652 | 3444.616 | 3444.616 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | ascii-off | 194.708 | 196.044 | 257.130 | 257.130 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | ascii-off | 2626.933 | 2678.811 | 4876.738 | 4876.738 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | ascii-off | 63298.173 | 64144.248 | 65364.992 | 65364.992 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "ascii": build 0.642 s; parses 148486; resident 587 MiB; peak resident 627 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
