# Musenalm query checkpoint

Code revision: `73b5f0338fd1a0c2122dea990d5f9921576d85c7`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q081 | ascii-off | 1091.146 | 1114.097 | 1580.393 | 1580.393 | 888 | 888 | 888 | 0/3 | ok |
| Q085 | ascii-off | 5164.956 | 11250.440 | 11260.265 | 11260.265 | 4925 | 4928 | 4928 | 0/3 | ok |

## Index construction

- "ascii": build 0.945 s; parses 148486; resident 458 MiB; peak resident 467 MiB; estimated index 66 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
