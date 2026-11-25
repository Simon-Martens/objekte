# Musenalm query checkpoint

Code revision: `732725bcfba34e11f6b85b0125e0e447151fb03a`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q155 | unicode-off | 241.610 | 244.085 | 340.155 | 340.155 | 3821 | 3821 | 0 | 0/7 | ok |
| Q163 | unicode-off | 5520.687 | 5874.363 | 8262.779 | 8262.779 | 1003 | 1627 | 1627 | 0/7 | ok |
| Q166 | unicode-off | 30392.888 | 30794.402 | 33712.840 | 33712.840 | 33619 | 0 | 0 | 0/7 | ok |
| Q170 | unicode-off | 45.062 | 48.354 | 126.211 | 126.211 | 1 | 1 | 2 | 0/7 | ok |
| Q195 | unicode-off | 28.050 | 28.459 | 41.018 | 41.018 | 0 | 0 | 0 | 0/7 | ok |
| Q219 | unicode-off | 159.517 | 160.361 | 198.507 | 198.507 | 3821 | 3821 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.760 s; parses 148486; resident 659 MiB; peak resident 699 MiB; estimated index 103 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
