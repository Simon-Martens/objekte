# Musenalm query checkpoint

Code revision: `efb0c73860dfc2ec519852bfc8f8116ededc00ed`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Status |
|---|---|---:|---:|---:|---:|---:|---:|---|
| Q002 | ascii-off | 184556.266 | 185481.649 | 190957.015 | 5608 | 6645 | 6645 | ok |
| Q006 | ascii-off | 2453812.660 | 2463338.822 | 2478699.709 | 65548 | 148486 | 148486 | ok |
| Q007 | ascii-off | 2556150.220 | 2577800.172 | 2594786.067 | 147798 | 148486 | 148486 | ok |
| Q013 | ascii-off | 298244.287 | 298874.986 | 301162.355 | 57 | 4010 | 4010 | ok |
| Q024 | ascii-off | 1929597.276 | 1943549.756 | 1977417.857 | 1927 | 133948 | 133948 | ok |

## Index construction

- "ascii": build 14.197 s; snapshot write 0.310 s; parses 148486; resident 241 MiB; estimated index 65 MiB; fresh-process restore median 0.544 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
