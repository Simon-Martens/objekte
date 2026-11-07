# Musenalm query checkpoint

Code revision: `64b5df7b68977b28501fccda45391d17461a0b42`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Status |
|---|---|---:|---:|---:|---:|---:|---:|---|
| Q031 | ascii-off | 66.634 | 70.735 | 137.642 | 203 | 203 | 0 | ok |
| Q070 | ascii-off | 844.439 | 869.653 | 1230.064 | 1 | 1 | 1 | ok |
| Q141 | ascii-off | 26139.728 | 26714.283 | 28810.726 | 823 | 982 | 982 | ok |
| Q031 | unicode-off | 81.419 | 93.425 | 154.789 | 203 | 203 | 0 | ok |
| Q070 | unicode-off | 947.806 | 953.172 | 1062.073 | 1 | 1 | 1 | ok |
| Q141 | unicode-off | 23268.182 | 24041.791 | 25959.238 | 823 | 842 | 842 | ok |

## Index construction

- "ascii": build 14.175 s; snapshot write 0.273 s; parses 148486; resident 241 MiB; estimated index 65 MiB; fresh-process restore median 0.535 s over 5 runs.
- "unicode": build 15.293 s; snapshot write 0.287 s; parses 148486; resident 290 MiB; estimated index 77 MiB; fresh-process restore median 0.553 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
