# Musenalm query checkpoint

Code revision: `e56f6654e9fb93a634f1c5ce1835689a78b864c2`. Repetitions: 30. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 5153.914 | 5719.306 | 7296.256 | 3269 | 4872 | 4872 | 0/30 | 0/30 | ok |
| Q008 | ascii-off | 3.276 | 3.908 | 42.545 | 0 | 0 | 0 | 0/30 | 0/30 | ok |
| Q042 | ascii-off | 26.803 | 29.558 | 81.039 | 1520 | 1520 | 0 | 0/30 | 0/30 | ok |
| Q143 | ascii-off | 2834.703 | 3009.401 | 4028.066 | 2057 | 2251 | 2251 | 0/30 | 0/30 | ok |
| Q167 | ascii-off | 2785.029 | 3050.687 | 4064.310 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q175 | ascii-off | 2524.500 | 2777.002 | 3654.634 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q177 | ascii-off | 2348.574 | 2451.494 | 3467.795 | 86564 | 0 | 0 | 0/30 | 0/30 | ok |
| Q194 | ascii-off | 160.785 | 194.699 | 798.946 | 7 | 203 | 203 | 0/30 | 0/30 | ok |
| Q202 | ascii-off | 87228.426 | 89058.587 | 103921.527 | 133948 | 133948 | 133948 | 0/30 | 0/30 | ok |
| Q001 | unicode-off | 5079.149 | 5830.560 | 9336.747 | 3269 | 4872 | 4872 | 0/30 | 0/30 | ok |
| Q008 | unicode-off | 3.459 | 3.879 | 46.728 | 0 | 0 | 0 | 0/30 | 0/30 | ok |
| Q042 | unicode-off | 26.908 | 29.134 | 73.388 | 1520 | 1520 | 0 | 0/30 | 0/30 | ok |
| Q143 | unicode-off | 2849.658 | 3081.686 | 3712.003 | 2057 | 2251 | 2251 | 0/30 | 0/30 | ok |
| Q167 | unicode-off | 2802.520 | 3101.292 | 4764.589 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q175 | unicode-off | 2471.661 | 2691.508 | 3756.236 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q177 | unicode-off | 2247.329 | 2462.602 | 3323.851 | 86564 | 0 | 0 | 0/30 | 0/30 | ok |
| Q194 | unicode-off | 163.221 | 186.103 | 236.048 | 7 | 203 | 203 | 0/30 | 0/30 | ok |
| Q202 | unicode-off | 86622.428 | 90152.622 | 97220.107 | 133948 | 133948 | 133948 | 0/30 | 0/30 | ok |

## Index construction

- "ascii": build 0.970 s; snapshot write 0.397 s; parses 148486; resident 578 MiB; peak resident 685 MiB; estimated index 67 MiB; snapshot 126 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.
- "unicode": build 0.992 s; snapshot write 0.387 s; parses 148486; resident 1098 MiB; peak resident 1171 MiB; estimated index 70 MiB; snapshot 128 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
