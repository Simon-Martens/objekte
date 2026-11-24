# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q031 | ascii-off | 25.137 | 25.995 | 101.910 | 101.910 | 203 | 203 | 0 | 0/7 | ok |
| Q061 | ascii-off | 26.349 | 28.120 | 76.758 | 76.758 | 1 | 1 | 1 | 0/7 | ok |
| Q067 | ascii-off | 72.242 | 74.241 | 241.423 | 241.423 | 1 | 1 | 0 | 0/7 | ok |
| Q099 | ascii-off | 187.687 | 190.438 | 268.082 | 268.082 | 59586 | 59586 | 0 | 0/7 | ok |
| Q104 | ascii-off | 47.256 | 49.395 | 97.104 | 97.104 | 106 | 106 | 0 | 0/7 | ok |
| Q111 | ascii-off | 124.868 | 130.052 | 155.962 | 155.962 | 4928 | 4928 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.643 s; parses 148486; resident 562 MiB; peak resident 602 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
