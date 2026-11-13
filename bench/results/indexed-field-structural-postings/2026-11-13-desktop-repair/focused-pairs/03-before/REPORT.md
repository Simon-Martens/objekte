# Musenalm query checkpoint

Code revision: `4cf5d759b9b1905a2063030e394969b05581763d`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14732.820 | 14983.260 | 17813.248 | 3269 | 4872 | 4872 | 6/7 | 0/7 | ok |
| Q052 | ascii-off | 3399.740 | 3887.524 | 6817.390 | 86764 | 86764 | 0 | 6/7 | 0/7 | ok |
| Q069 | ascii-off | 6563.534 | 6767.365 | 9212.776 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q112 | ascii-off | 6325.138 | 6391.517 | 6988.111 | 25 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q116 | ascii-off | 2289.356 | 2573.668 | 6311.263 | 70428 | 70428 | 0 | 6/7 | 0/7 | ok |
| Q117 | ascii-off | 2059.168 | 2365.018 | 3937.252 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q167 | ascii-off | 4941.281 | 5058.232 | 6144.224 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q168 | ascii-off | 4364.098 | 4422.072 | 6230.435 | 350 | 1202 | 2385 | 6/7 | 0/7 | ok |
| Q174 | ascii-off | 22882.229 | 23088.497 | 24418.512 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q175 | ascii-off | 4357.506 | 4576.857 | 5149.107 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 4409.123 | 4645.383 | 7035.232 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q189 | ascii-off | 503.377 | 571.225 | 1505.244 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q195 | ascii-off | 6232.690 | 6813.314 | 7101.069 | 0 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q196 | ascii-off | 6685.522 | 6724.624 | 6913.227 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q214 | ascii-off | 2777.914 | 2952.879 | 13194.809 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q001 | unicode-off | 14596.894 | 14713.451 | 17105.746 | 3269 | 4872 | 4872 | 6/7 | 0/7 | ok |
| Q052 | unicode-off | 3388.980 | 3589.064 | 13977.436 | 86764 | 86764 | 0 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 6546.808 | 6697.229 | 10271.522 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q112 | unicode-off | 6398.981 | 6488.782 | 7126.828 | 25 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q116 | unicode-off | 2444.340 | 2710.103 | 6537.448 | 70428 | 70428 | 0 | 6/7 | 0/7 | ok |
| Q117 | unicode-off | 2150.124 | 2406.282 | 3964.782 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q167 | unicode-off | 4774.839 | 5054.700 | 6176.774 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q168 | unicode-off | 4431.120 | 4574.526 | 5593.605 | 350 | 1202 | 2385 | 6/7 | 0/7 | ok |
| Q174 | unicode-off | 23140.461 | 23336.891 | 24465.461 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q175 | unicode-off | 4449.598 | 4667.206 | 5341.014 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 4495.871 | 4758.237 | 6813.000 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q189 | unicode-off | 543.497 | 578.597 | 1821.138 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q195 | unicode-off | 6348.191 | 6472.172 | 7130.598 | 0 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q196 | unicode-off | 6730.393 | 6758.708 | 6886.574 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q214 | unicode-off | 2351.156 | 3027.364 | 6202.746 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 1.006 s; snapshot write 0.280 s; parses 148486; resident 492 MiB; peak resident 515 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.608/0.640/0.692 s over 5 runs.
- "unicode": build 1.050 s; snapshot write 0.255 s; parses 148486; resident 674 MiB; peak resident 674 MiB; estimated index 69 MiB; snapshot 126 MiB; fresh-process restore min/median/max 0.609/0.614/0.672 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
