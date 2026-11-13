# Musenalm query checkpoint

Code revision: `a7a31feb5da625008c8e0e979de32bdcb64a80cd`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 5457.216 | 6041.138 | 7249.544 | 3269 | 4872 | 4872 | 0/7 | 0/7 | ok |
| Q003 | ascii-off | 3638.984 | 4183.107 | 4499.095 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | ascii-off | 9076.238 | 14211.842 | 24828.150 | 2995 | 7766 | 7766 | 0/7 | 0/7 | ok |
| Q028 | ascii-off | 2467.590 | 2638.643 | 3196.540 | 4177 | 4177 | 4177 | 0/7 | 0/7 | ok |
| Q069 | ascii-off | 2713.460 | 2815.156 | 3635.675 | 4010 | 4010 | 4010 | 0/7 | 0/7 | ok |
| Q100 | ascii-off | 13191.639 | 14074.337 | 15315.230 | 0 | 22185 | 22185 | 0/7 | 0/7 | ok |
| Q190 | ascii-off | 4124.630 | 4321.733 | 4741.889 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |
| Q001 | unicode-off | 5922.262 | 7948.170 | 10554.287 | 3269 | 4872 | 4872 | 0/7 | 0/7 | ok |
| Q003 | unicode-off | 3737.316 | 3926.439 | 4925.451 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | unicode-off | 9542.119 | 14344.862 | 27791.797 | 2995 | 7773 | 7773 | 0/7 | 0/7 | ok |
| Q028 | unicode-off | 2772.790 | 3179.186 | 3649.465 | 4177 | 4177 | 4177 | 0/7 | 0/7 | ok |
| Q069 | unicode-off | 2791.220 | 3090.616 | 3714.727 | 4010 | 4010 | 4010 | 0/7 | 0/7 | ok |
| Q100 | unicode-off | 13553.635 | 13993.307 | 15130.087 | 0 | 22570 | 22570 | 0/7 | 0/7 | ok |
| Q190 | unicode-off | 4237.758 | 4528.912 | 5072.429 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |

## Index construction

- "ascii": build 0.963 s; snapshot write 0.493 s; parses 148486; resident 583 MiB; peak resident 748 MiB; estimated index 66 MiB; snapshot 170 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.
- "unicode": build 1.017 s; snapshot write 0.460 s; parses 148486; resident 1093 MiB; peak resident 1222 MiB; estimated index 69 MiB; snapshot 172 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
