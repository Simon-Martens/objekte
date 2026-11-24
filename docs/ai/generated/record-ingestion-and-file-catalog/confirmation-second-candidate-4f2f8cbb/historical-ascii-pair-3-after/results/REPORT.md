# Musenalm query checkpoint

Code revision: `4f2f8cbb3f88ca4e279509d9d2b60052312467c4`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q050 | ascii-off | 273.112 | 277.196 | 413.870 | 413.870 | 81 | 81 | 0 | 0/7 | ok |
| Q051 | ascii-off | 106.447 | 108.025 | 153.431 | 153.431 | 1942 | 1942 | 0 | 0/7 | ok |
| Q057 | ascii-off | 16.713 | 17.146 | 34.709 | 34.709 | 4177 | 4177 | 0 | 0/7 | ok |
| Q064 | ascii-off | 310.360 | 312.721 | 483.013 | 483.013 | 1 | 210 | 0 | 0/7 | ok |
| Q067 | ascii-off | 69.967 | 73.824 | 109.651 | 109.651 | 1 | 1 | 0 | 0/7 | ok |
| Q090 | ascii-off | 6.646 | 6.964 | 14.066 | 14.066 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.629 s; parses 148486; resident 570 MiB; peak resident 610 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
