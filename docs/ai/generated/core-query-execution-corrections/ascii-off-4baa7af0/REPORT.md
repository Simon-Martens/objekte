# Musenalm query checkpoint

Code revision: `4baa7af067f74f5ffb5772ef641ff0994fc4173f`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14597.712 | 14791.731 | 20834.553 | 20834.553 | 3269 | 4872 | 4872 | 0/5 | ok |
| Q007 | ascii-off | 338291.651 | 338587.789 | 349108.099 | 349108.099 | 147798 | 148486 | 148486 | 0/5 | ok |
| Q030 | ascii-off | 2916.693 | 3002.466 | 3337.514 | 3337.514 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q035 | ascii-off | 54.519 | 59.028 | 134.623 | 134.623 | 124 | 124 | 0 | 0/5 | ok |
| Q084 | ascii-off | 56.768 | 57.706 | 75.417 | 75.417 | 4922 | 4922 | 0 | 0/5 | ok |
| Q127 | ascii-off | 71.042 | 73.242 | 98.774 | 98.774 | 203 | 203 | 0 | 0/5 | ok |
| Q141 | ascii-off | 4416.713 | 4653.902 | 4725.704 | 4725.704 | 823 | 982 | 982 | 0/5 | ok |
| Q150 | ascii-off | 2691.066 | 2765.483 | 3029.471 | 3029.471 | 390 | 457 | 457 | 0/5 | ok |
| Q163 | ascii-off | 2070.155 | 2136.138 | 2264.161 | 2264.161 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q164 | ascii-off | 6419.335 | 6549.547 | 6712.087 | 6712.087 | 4928 | 4928 | 4928 | 0/5 | ok |
| Q181 | ascii-off | 33850.402 | 34864.847 | 35320.414 | 35320.414 | 6 | 4928 | 8838 | 0/5 | ok |

## Index construction

- "ascii": build 0.751 s; parses 148486; resident 665 MiB; peak resident 707 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
