# Musenalm query checkpoint

Code revision: `4cf5d759b9b1905a2063030e394969b05581763d`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14713.528 | 14989.206 | 24039.689 | 3269 | 4872 | 4872 | 6/7 | 0/7 | ok |
| Q052 | ascii-off | 3331.040 | 3972.236 | 6807.257 | 86764 | 86764 | 0 | 6/7 | 0/7 | ok |
| Q069 | ascii-off | 6584.762 | 6649.057 | 9241.419 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q112 | ascii-off | 6302.135 | 6864.959 | 7584.655 | 25 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q116 | ascii-off | 2428.913 | 2784.445 | 8449.175 | 70428 | 70428 | 0 | 6/7 | 0/7 | ok |
| Q117 | ascii-off | 2214.657 | 2395.575 | 3913.699 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q167 | ascii-off | 4824.164 | 5097.272 | 6047.126 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q168 | ascii-off | 4449.388 | 4876.746 | 7346.013 | 350 | 1202 | 2385 | 6/7 | 0/7 | ok |
| Q174 | ascii-off | 23321.423 | 25586.245 | 27192.576 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q175 | ascii-off | 4513.908 | 4614.445 | 4993.522 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 4376.336 | 4652.537 | 7495.461 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q189 | ascii-off | 547.719 | 717.093 | 1588.621 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q195 | ascii-off | 6147.157 | 6597.724 | 8355.059 | 0 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q196 | ascii-off | 6605.454 | 6625.442 | 6801.806 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q214 | ascii-off | 3082.379 | 3341.706 | 7282.193 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q001 | unicode-off | 14718.805 | 15254.541 | 23383.596 | 3269 | 4872 | 4872 | 6/7 | 0/7 | ok |
| Q052 | unicode-off | 3548.127 | 3633.962 | 7051.972 | 86764 | 86764 | 0 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 6479.183 | 6584.076 | 8301.100 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q112 | unicode-off | 6288.199 | 6350.623 | 6934.556 | 25 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q116 | unicode-off | 2259.634 | 2879.206 | 6418.949 | 70428 | 70428 | 0 | 6/7 | 0/7 | ok |
| Q117 | unicode-off | 2081.963 | 2302.056 | 3868.271 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q167 | unicode-off | 4930.192 | 5228.195 | 6268.489 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q168 | unicode-off | 4512.221 | 4608.628 | 6464.529 | 350 | 1202 | 2385 | 6/7 | 0/7 | ok |
| Q174 | unicode-off | 22785.452 | 23105.787 | 24110.679 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q175 | unicode-off | 4334.459 | 4655.992 | 5211.065 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 4441.509 | 4770.273 | 6770.289 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q189 | unicode-off | 507.913 | 521.024 | 1891.669 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q195 | unicode-off | 6114.964 | 6223.126 | 6843.931 | 0 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q196 | unicode-off | 6612.969 | 6639.232 | 6749.109 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q214 | unicode-off | 2841.064 | 3013.981 | 8441.676 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 0.989 s; snapshot write 0.319 s; parses 148486; resident 486 MiB; peak resident 509 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.608/0.652/0.687 s over 5 runs.
- "unicode": build 0.983 s; snapshot write 0.284 s; parses 148486; resident 678 MiB; peak resident 678 MiB; estimated index 69 MiB; snapshot 126 MiB; fresh-process restore min/median/max 0.610/0.637/0.681 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
