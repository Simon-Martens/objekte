# Musenalm query checkpoint

Code revision: `9ae617b5df3f0a082aa923195a28019d5c36815d`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q102 | ascii-off | 35.497 | 42.416 | 112.858 | 112.858 | 0 | 0 | 0 | 0/3 | ok |
| Q122 | ascii-off | 56.395 | 59.841 | 94.298 | 94.298 | 67 | 67 | 0 | 0/3 | ok |

## Index construction

- "ascii": build 0.726 s; parses 148486; resident 662 MiB; peak resident 703 MiB; estimated index 106 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
