# Musenalm query checkpoint

Code revision: `20261604bebf949131441c8262f1f442bce4207c`. Repetitions: 31. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q056 | ascii-off | 785.457 | 866.588 | 2589.536 | 11672 | 11672 | 0 | 30/31 | 0/31 | ok |
| Q085 | ascii-off | 4040.278 | 5107.916 | 13621.579 | 4925 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q099 | ascii-off | 1810.477 | 2818.887 | 16574.969 | 59586 | 59586 | 0 | 30/31 | 0/31 | ok |
| Q107 | ascii-off | 11351.136 | 13260.959 | 24585.289 | 2684 | 4010 | 4010 | 30/31 | 0/31 | ok |
| Q112 | ascii-off | 2997.561 | 4374.939 | 13037.084 | 25 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q056 | unicode-off | 789.160 | 888.799 | 2505.943 | 11672 | 11672 | 0 | 30/31 | 0/31 | ok |
| Q085 | unicode-off | 4301.447 | 5034.129 | 16687.252 | 4925 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q099 | unicode-off | 1913.428 | 3929.417 | 16509.406 | 59586 | 59586 | 0 | 30/31 | 0/31 | ok |
| Q107 | unicode-off | 10682.338 | 13199.881 | 24809.060 | 2684 | 4010 | 4010 | 30/31 | 0/31 | ok |
| Q112 | unicode-off | 3023.513 | 4137.323 | 9315.508 | 25 | 4928 | 4928 | 30/31 | 0/31 | ok |

## Index construction

- "ascii": build 11.027 s; snapshot write 0.271 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.488/0.494/0.533 s over 5 runs.
- "unicode": build 12.171 s; snapshot write 0.303 s; parses 148486; resident 331 MiB; peak resident 403 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.511/0.520/0.544 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
