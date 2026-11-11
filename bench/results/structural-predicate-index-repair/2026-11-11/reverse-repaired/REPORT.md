# Musenalm query checkpoint

Code revision: `95f1736d448a01d94650afd237149c158c0e7a8a`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | ascii-off | 25668.878 | 28145.789 | 50387.942 | 133948 | 133948 | 0 | 10/11 | 0/11 | ok |
| Q141 | ascii-off | 7897.348 | 11988.680 | 37974.083 | 823 | 982 | 982 | 10/11 | 0/11 | ok |
| Q190 | ascii-off | 20636.338 | 22947.378 | 37113.577 | 3821 | 3821 | 3821 | 10/11 | 0/11 | ok |
| Q119 | unicode-off | 25195.910 | 26648.777 | 39139.401 | 133948 | 133948 | 0 | 10/11 | 0/11 | ok |
| Q141 | unicode-off | 7488.150 | 9129.643 | 17974.220 | 823 | 982 | 982 | 10/11 | 0/11 | ok |
| Q190 | unicode-off | 20892.299 | 25958.381 | 37865.138 | 3821 | 3821 | 3821 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 2.549 s; snapshot write 0.429 s; parses 148486; resident 539 MiB; peak resident 565 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.320/1.326/1.362 s over 5 runs.
- "unicode": build 2.574 s; snapshot write 0.447 s; parses 148486; resident 704 MiB; peak resident 704 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.330/1.338/1.406 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
