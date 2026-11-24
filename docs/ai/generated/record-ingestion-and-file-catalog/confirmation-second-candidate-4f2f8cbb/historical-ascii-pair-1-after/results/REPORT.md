# Musenalm query checkpoint

Code revision: `4f2f8cbb3f88ca4e279509d9d2b60052312467c4`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q050 | ascii-off | 272.497 | 279.618 | 413.835 | 413.835 | 81 | 81 | 0 | 0/7 | ok |
| Q051 | ascii-off | 103.771 | 107.260 | 156.603 | 156.603 | 1942 | 1942 | 0 | 0/7 | ok |
| Q057 | ascii-off | 16.934 | 17.151 | 33.145 | 33.145 | 4177 | 4177 | 0 | 0/7 | ok |
| Q064 | ascii-off | 308.601 | 322.520 | 472.236 | 472.236 | 1 | 210 | 0 | 0/7 | ok |
| Q067 | ascii-off | 69.130 | 74.807 | 108.925 | 108.925 | 1 | 1 | 0 | 0/7 | ok |
| Q090 | ascii-off | 6.687 | 7.012 | 13.898 | 13.898 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.626 s; parses 148486; resident 587 MiB; peak resident 626 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
