# Musenalm query checkpoint

Code revision: `c54afc3eb1cdc496734bfb30bcc9a3ea38988b78`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 5637.080 | 7421.207 | 11493.489 | 3269 | 4872 | 4872 | 0/7 | 0/7 | ok |
| Q003 | ascii-off | 3540.290 | 3902.884 | 4687.622 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | ascii-off | 8472.848 | 12022.926 | 24950.899 | 2995 | 7766 | 7766 | 0/7 | 0/7 | ok |
| Q028 | ascii-off | 2643.249 | 2946.329 | 5482.843 | 4177 | 4177 | 4177 | 0/7 | 0/7 | ok |
| Q069 | ascii-off | 2733.417 | 2859.833 | 3353.734 | 4010 | 4010 | 4010 | 0/7 | 0/7 | ok |
| Q100 | ascii-off | 13467.187 | 13741.762 | 15846.867 | 0 | 22185 | 22185 | 0/7 | 0/7 | ok |
| Q190 | ascii-off | 4124.233 | 4265.349 | 4701.444 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |
| Q001 | unicode-off | 5687.051 | 6805.336 | 7733.904 | 3269 | 4872 | 4872 | 0/7 | 0/7 | ok |
| Q003 | unicode-off | 3529.043 | 3718.615 | 4396.574 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | unicode-off | 12508.190 | 14429.205 | 26973.653 | 2995 | 7773 | 7773 | 0/7 | 0/7 | ok |
| Q028 | unicode-off | 2415.182 | 2876.533 | 3353.293 | 4177 | 4177 | 4177 | 0/7 | 0/7 | ok |
| Q069 | unicode-off | 2757.868 | 3124.079 | 3423.766 | 4010 | 4010 | 4010 | 0/7 | 0/7 | ok |
| Q100 | unicode-off | 13182.122 | 14012.359 | 15590.609 | 0 | 22570 | 22570 | 0/7 | 0/7 | ok |
| Q190 | unicode-off | 4249.992 | 4621.643 | 4904.135 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |

## Index construction

- "ascii": build 0.965 s; snapshot write 0.468 s; parses 148486; resident 587 MiB; peak resident 751 MiB; estimated index 66 MiB; snapshot 170 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.
- "unicode": build 1.045 s; snapshot write 0.484 s; parses 148486; resident 1099 MiB; peak resident 1228 MiB; estimated index 69 MiB; snapshot 172 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
