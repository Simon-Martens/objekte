# Musenalm query checkpoint

Code revision: `4cf5d759b9b1905a2063030e394969b05581763d`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14820.851 | 15006.226 | 19201.479 | 3269 | 4872 | 4872 | 6/7 | 0/7 | ok |
| Q052 | ascii-off | 3419.944 | 3715.576 | 6946.518 | 86764 | 86764 | 0 | 6/7 | 0/7 | ok |
| Q069 | ascii-off | 6450.180 | 6630.786 | 8727.906 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q112 | ascii-off | 6240.670 | 6286.875 | 6967.960 | 25 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q116 | ascii-off | 2308.253 | 2640.698 | 6431.367 | 70428 | 70428 | 0 | 6/7 | 0/7 | ok |
| Q117 | ascii-off | 2064.352 | 2252.252 | 3735.818 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q167 | ascii-off | 4931.845 | 5060.918 | 6338.516 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q168 | ascii-off | 4394.007 | 4431.398 | 6655.671 | 350 | 1202 | 2385 | 6/7 | 0/7 | ok |
| Q174 | ascii-off | 22548.170 | 23144.742 | 24192.105 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q175 | ascii-off | 4553.643 | 4592.412 | 5011.625 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 4370.406 | 4613.413 | 7371.269 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q189 | ascii-off | 543.420 | 564.186 | 2404.615 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q195 | ascii-off | 6224.821 | 6259.694 | 7033.244 | 0 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q196 | ascii-off | 6640.610 | 6700.410 | 6989.046 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q214 | ascii-off | 2781.762 | 3445.300 | 8564.201 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q001 | unicode-off | 14796.315 | 15195.004 | 18766.606 | 3269 | 4872 | 4872 | 6/7 | 0/7 | ok |
| Q052 | unicode-off | 3280.374 | 3562.735 | 6733.703 | 86764 | 86764 | 0 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 6477.831 | 6685.089 | 8118.938 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q112 | unicode-off | 6286.711 | 6379.022 | 6987.061 | 25 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q116 | unicode-off | 2281.767 | 2684.490 | 6584.526 | 70428 | 70428 | 0 | 6/7 | 0/7 | ok |
| Q117 | unicode-off | 2150.991 | 2353.550 | 4045.677 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q167 | unicode-off | 4772.450 | 5301.862 | 6110.800 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q168 | unicode-off | 4440.332 | 4614.330 | 5777.867 | 350 | 1202 | 2385 | 6/7 | 0/7 | ok |
| Q174 | unicode-off | 22974.429 | 23224.782 | 24874.264 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q175 | unicode-off | 4283.536 | 4514.889 | 5207.358 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 4417.832 | 4828.053 | 7086.965 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q189 | unicode-off | 495.899 | 547.873 | 1283.338 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q195 | unicode-off | 6193.430 | 6301.390 | 7047.726 | 0 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q196 | unicode-off | 6685.194 | 6704.169 | 6837.863 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q214 | unicode-off | 2606.942 | 3270.253 | 5916.899 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 1.038 s; snapshot write 0.243 s; parses 148486; resident 491 MiB; peak resident 514 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.607/0.642/0.686 s over 5 runs.
- "unicode": build 1.057 s; snapshot write 0.247 s; parses 148486; resident 689 MiB; peak resident 689 MiB; estimated index 69 MiB; snapshot 126 MiB; fresh-process restore min/median/max 0.614/0.631/0.683 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
