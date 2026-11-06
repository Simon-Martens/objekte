# Paired FTS case-mode measurements

Measured core: `ff759b225c6bc4bfdd614c542fe38fe017a900b6`. Each alphabet used one loaded repository; five timed repetitions per request. Verified membership cache is off or warm at 64 MiB. Plans were obtained outside timed samples. FILTER-only rows have no FTS case behavior; their two labels are controls.

| Alphabet | Cache | Case | FTS | Field | FILTER | Exact mode | Min ms | Median ms | Max ms | Candidates | Opens | Scanned | Parses | Matches | Page | Stores | Gram keys |
|---|---|---|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| ascii | off | raw_fts | Der | — | — | sensitive | 111.242 | 112.836 | 114.143 | 89586 | 89586 | 89586 | 89586 | 19681 | 10 | 15 | 25477 |
| ascii | off | raw_fts | Der | — | — | insensitive | 177.012 | 178.498 | 189.760 | 89586 | 89586 | 89586 | 89586 | 80871 | 10 | 15 | 25477 |
| ascii | off | raw_fts | Liebe | — | — | sensitive | 21.413 | 22.130 | 28.895 | 13757 | 13757 | 13757 | 13757 | 5589 | 10 | 15 | 25477 |
| ascii | off | raw_fts | Liebe | — | — | insensitive | 31.470 | 40.224 | 43.765 | 13757 | 13757 | 13757 | 13757 | 8017 | 10 | 15 | 25477 |
| ascii | off | raw_fts | Goethe | — | — | sensitive | 7.164 | 7.651 | 15.304 | 1262 | 1262 | 1262 | 1262 | 516 | 10 | 15 | 25477 |
| ascii | off | raw_fts | Goethe | — | — | insensitive | 11.185 | 12.536 | 13.817 | 1262 | 1262 | 1262 | 1262 | 541 | 10 | 15 | 25477 |
| ascii | off | raw_fts | Müller | — | — | sensitive | 30.459 | 32.466 | 43.539 | 15219 | 15219 | 15219 | 15219 | 1164 | 10 | 15 | 25477 |
| ascii | off | raw_fts | Müller | — | — | insensitive | 46.520 | 48.741 | 71.460 | 15219 | 15219 | 15219 | 15219 | 1221 | 10 | 15 | 25477 |
| ascii | off | raw_fts | Frühling | — | — | sensitive | 17.826 | 18.806 | 24.853 | 6668 | 6668 | 6668 | 6668 | 1966 | 10 | 15 | 25477 |
| ascii | off | raw_fts | Frühling | — | — | insensitive | 30.111 | 32.942 | 39.406 | 6668 | 6668 | 6668 | 6668 | 2030 | 10 | 15 | 25477 |
| ascii | off | raw_fts | ß | — | — | sensitive | 80.459 | 83.087 | 123.223 | 55544 | 55544 | 55544 | 55544 | 33078 | 10 | 15 | 25477 |
| ascii | off | raw_fts | ß | — | — | insensitive | 119.352 | 120.624 | 150.612 | 55544 | 55544 | 55544 | 55544 | 55544 | 10 | 15 | 25477 |
| ascii | off | raw_fts | ss | — | — | sensitive | 77.931 | 83.306 | 113.127 | 55544 | 55544 | 55544 | 55544 | 28648 | 10 | 15 | 25477 |
| ascii | off | raw_fts | ss | — | — | insensitive | 119.844 | 120.308 | 155.801 | 55544 | 55544 | 55544 | 55544 | 55544 | 10 | 15 | 25477 |
| ascii | off | selected_name | Goethe | name | — | sensitive | 0.113 | 0.118 | 0.124 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 25477 |
| ascii | off | selected_name | Goethe | name | — | insensitive | 0.111 | 0.112 | 0.115 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 25477 |
| ascii | off | selected_folded | GOETHE | name:folded | — | sensitive | 0.112 | 0.112 | 0.117 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 25477 |
| ascii | off | selected_folded | GOETHE | name:folded | — | insensitive | 0.111 | 0.111 | 0.114 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 25477 |
| ascii | off | selected_title | FRÜHLING | preferred_title:lower | — | sensitive | 0.317 | 0.359 | 0.390 | 322 | 322 | 322 | 322 | 11 | 10 | 15 | 25477 |
| ascii | off | selected_title | FRÜHLING | preferred_title:lower | — | insensitive | 0.365 | 0.380 | 0.849 | 322 | 322 | 322 | 322 | 11 | 10 | 15 | 25477 |
| ascii | off | filter_name | — | — | name ~ 'Goethe' | sensitive | 0.096 | 0.127 | 0.150 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 25477 |
| ascii | off | filter_name | — | — | name ~ 'Goethe' | insensitive | 0.094 | 0.120 | 0.122 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 25477 |
| ascii | off | filter_lower | — | — | name:lower ~ 'goethe' | sensitive | 0.073 | 0.115 | 0.123 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 25477 |
| ascii | off | filter_lower | — | — | name:lower ~ 'goethe' | insensitive | 0.104 | 0.110 | 0.118 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 25477 |
| ascii | off | filter_folded | — | — | name:folded ~ 'GOETHE' | sensitive | 0.109 | 0.111 | 0.113 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 25477 |
| ascii | off | filter_folded | — | — | name:folded ~ 'GOETHE' | insensitive | 0.077 | 0.109 | 0.110 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 25477 |
| ascii | off | filter_title_lower | — | — | preferred_title:lower ~ 'FRÜHLING' | sensitive | 0.323 | 0.359 | 0.452 | 322 | 322 | 322 | 322 | 11 | 10 | 15 | 25477 |
| ascii | off | filter_title_lower | — | — | preferred_title:lower ~ 'FRÜHLING' | insensitive | 0.330 | 0.369 | 0.388 | 322 | 322 | 322 | 322 | 11 | 10 | 15 | 25477 |
| ascii | off | combined | Goethe | — | name:lower ~ 'goethe' | sensitive | 0.520 | 0.565 | 0.612 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 25477 |
| ascii | off | combined | Goethe | — | name:lower ~ 'goethe' | insensitive | 0.847 | 0.849 | 0.855 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 25477 |
| ascii | off | exact_posting_control | — | — | name = 'Johann Wolfgang Goethe' | sensitive | 0.047 | 0.086 | 0.087 | 0 | 0 | 0 | 0 | 0 | 0 | 15 | 25477 |
| ascii | off | exact_posting_control | — | — | name = 'Johann Wolfgang Goethe' | insensitive | 0.064 | 0.087 | 0.088 | 0 | 0 | 0 | 0 | 0 | 0 | 15 | 25477 |
| ascii | warm | raw_fts | Der | — | — | sensitive | 0.003 | 0.003 | 0.014 | 0 | 0 | 0 | 0 | 19681 | 10 | 15 | 25477 |
| ascii | warm | raw_fts | Der | — | — | insensitive | 0.003 | 0.003 | 0.018 | 0 | 0 | 0 | 0 | 80871 | 10 | 15 | 25477 |
| ascii | warm | raw_fts | Liebe | — | — | sensitive | 0.003 | 0.003 | 0.014 | 0 | 0 | 0 | 0 | 5589 | 10 | 15 | 25477 |
| ascii | warm | raw_fts | Liebe | — | — | insensitive | 0.003 | 0.004 | 0.024 | 0 | 0 | 0 | 0 | 8017 | 10 | 15 | 25477 |
| ascii | warm | raw_fts | Goethe | — | — | sensitive | 0.006 | 0.007 | 0.032 | 0 | 0 | 0 | 0 | 516 | 10 | 15 | 25477 |
| ascii | warm | raw_fts | Goethe | — | — | insensitive | 0.004 | 0.005 | 0.042 | 0 | 0 | 0 | 0 | 541 | 10 | 15 | 25477 |
| ascii | warm | raw_fts | Müller | — | — | sensitive | 0.003 | 0.004 | 0.016 | 0 | 0 | 0 | 0 | 1164 | 10 | 15 | 25477 |
| ascii | warm | raw_fts | Müller | — | — | insensitive | 0.005 | 0.005 | 0.039 | 0 | 0 | 0 | 0 | 1221 | 10 | 15 | 25477 |
| ascii | warm | raw_fts | Frühling | — | — | sensitive | 0.004 | 0.005 | 0.022 | 0 | 0 | 0 | 0 | 1966 | 10 | 15 | 25477 |
| ascii | warm | raw_fts | Frühling | — | — | insensitive | 0.004 | 0.005 | 0.019 | 0 | 0 | 0 | 0 | 2030 | 10 | 15 | 25477 |
| ascii | warm | raw_fts | ß | — | — | sensitive | 0.003 | 0.004 | 0.015 | 0 | 0 | 0 | 0 | 33078 | 10 | 15 | 25477 |
| ascii | warm | raw_fts | ß | — | — | insensitive | 0.003 | 0.004 | 0.018 | 0 | 0 | 0 | 0 | 55544 | 10 | 15 | 25477 |
| ascii | warm | raw_fts | ss | — | — | sensitive | 0.006 | 0.007 | 0.023 | 0 | 0 | 0 | 0 | 28648 | 10 | 15 | 25477 |
| ascii | warm | raw_fts | ss | — | — | insensitive | 0.003 | 0.004 | 0.016 | 0 | 0 | 0 | 0 | 55544 | 10 | 15 | 25477 |
| ascii | warm | selected_name | Goethe | name | — | sensitive | 0.002 | 0.002 | 0.002 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 25477 |
| ascii | warm | selected_name | Goethe | name | — | insensitive | 0.001 | 0.001 | 0.002 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 25477 |
| ascii | warm | selected_folded | GOETHE | name:folded | — | sensitive | 0.001 | 0.001 | 0.002 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 25477 |
| ascii | warm | selected_folded | GOETHE | name:folded | — | insensitive | 0.001 | 0.001 | 0.001 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 25477 |
| ascii | warm | selected_title | FRÜHLING | preferred_title:lower | — | sensitive | 0.002 | 0.002 | 0.007 | 0 | 0 | 0 | 0 | 11 | 10 | 15 | 25477 |
| ascii | warm | selected_title | FRÜHLING | preferred_title:lower | — | insensitive | 0.002 | 0.004 | 0.006 | 0 | 0 | 0 | 0 | 11 | 10 | 15 | 25477 |
| ascii | warm | filter_name | — | — | name ~ 'Goethe' | sensitive | 0.001 | 0.001 | 0.002 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 25477 |
| ascii | warm | filter_name | — | — | name ~ 'Goethe' | insensitive | 0.001 | 0.001 | 0.002 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 25477 |
| ascii | warm | filter_lower | — | — | name:lower ~ 'goethe' | sensitive | 0.001 | 0.001 | 0.002 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 25477 |
| ascii | warm | filter_lower | — | — | name:lower ~ 'goethe' | insensitive | 0.001 | 0.001 | 0.001 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 25477 |
| ascii | warm | filter_folded | — | — | name:folded ~ 'GOETHE' | sensitive | 0.001 | 0.001 | 0.002 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 25477 |
| ascii | warm | filter_folded | — | — | name:folded ~ 'GOETHE' | insensitive | 0.001 | 0.001 | 0.002 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 25477 |
| ascii | warm | filter_title_lower | — | — | preferred_title:lower ~ 'FRÜHLING' | sensitive | 0.002 | 0.002 | 0.006 | 0 | 0 | 0 | 0 | 11 | 10 | 15 | 25477 |
| ascii | warm | filter_title_lower | — | — | preferred_title:lower ~ 'FRÜHLING' | insensitive | 0.002 | 0.002 | 0.003 | 0 | 0 | 0 | 0 | 11 | 10 | 15 | 25477 |
| ascii | warm | combined | Goethe | — | name:lower ~ 'goethe' | sensitive | 0.002 | 0.002 | 0.004 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 25477 |
| ascii | warm | combined | Goethe | — | name:lower ~ 'goethe' | insensitive | 0.002 | 0.002 | 0.003 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 25477 |
| ascii | warm | exact_posting_control | — | — | name = 'Johann Wolfgang Goethe' | sensitive | 0.001 | 0.001 | 0.002 | 0 | 0 | 0 | 0 | 0 | 0 | 15 | 25477 |
| ascii | warm | exact_posting_control | — | — | name = 'Johann Wolfgang Goethe' | insensitive | 0.001 | 0.001 | 0.001 | 0 | 0 | 0 | 0 | 0 | 0 | 15 | 25477 |
| unicode | off | raw_fts | Der | — | — | sensitive | 109.431 | 110.267 | 113.449 | 89753 | 89753 | 89753 | 89753 | 19681 | 10 | 15 | 37909 |
| unicode | off | raw_fts | Der | — | — | insensitive | 176.903 | 184.098 | 184.989 | 89753 | 89753 | 89753 | 89753 | 80871 | 10 | 15 | 37909 |
| unicode | off | raw_fts | Liebe | — | — | sensitive | 21.316 | 23.863 | 32.133 | 13757 | 13757 | 13757 | 13757 | 5589 | 10 | 15 | 37909 |
| unicode | off | raw_fts | Liebe | — | — | insensitive | 34.298 | 40.548 | 55.106 | 13757 | 13757 | 13757 | 13757 | 8017 | 10 | 15 | 37909 |
| unicode | off | raw_fts | Goethe | — | — | sensitive | 7.136 | 7.696 | 7.823 | 1262 | 1262 | 1262 | 1262 | 516 | 10 | 15 | 37909 |
| unicode | off | raw_fts | Goethe | — | — | insensitive | 12.031 | 12.368 | 16.845 | 1262 | 1262 | 1262 | 1262 | 541 | 10 | 15 | 37909 |
| unicode | off | raw_fts | Müller | — | — | sensitive | 4.725 | 5.002 | 5.692 | 1257 | 1257 | 1257 | 1257 | 1164 | 10 | 15 | 37909 |
| unicode | off | raw_fts | Müller | — | — | insensitive | 5.600 | 8.205 | 12.385 | 1257 | 1257 | 1257 | 1257 | 1221 | 10 | 15 | 37909 |
| unicode | off | raw_fts | Frühling | — | — | sensitive | 5.270 | 6.267 | 6.606 | 2113 | 2113 | 2113 | 2113 | 1966 | 10 | 15 | 37909 |
| unicode | off | raw_fts | Frühling | — | — | insensitive | 6.336 | 7.420 | 10.660 | 2113 | 2113 | 2113 | 2113 | 2030 | 10 | 15 | 37909 |
| unicode | off | raw_fts | ß | — | — | sensitive | 82.140 | 88.967 | 124.972 | 55544 | 55544 | 55544 | 55544 | 33078 | 10 | 15 | 37909 |
| unicode | off | raw_fts | ß | — | — | insensitive | 122.115 | 124.516 | 159.247 | 55544 | 55544 | 55544 | 55544 | 55544 | 10 | 15 | 37909 |
| unicode | off | raw_fts | ss | — | — | sensitive | 80.404 | 85.594 | 114.517 | 55544 | 55544 | 55544 | 55544 | 28648 | 10 | 15 | 37909 |
| unicode | off | raw_fts | ss | — | — | insensitive | 120.007 | 122.952 | 157.858 | 55544 | 55544 | 55544 | 55544 | 55544 | 10 | 15 | 37909 |
| unicode | off | selected_name | Goethe | name | — | sensitive | 0.079 | 0.120 | 0.131 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 37909 |
| unicode | off | selected_name | Goethe | name | — | insensitive | 0.097 | 0.114 | 0.115 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 37909 |
| unicode | off | selected_folded | GOETHE | name:folded | — | sensitive | 0.088 | 0.109 | 0.113 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 37909 |
| unicode | off | selected_folded | GOETHE | name:folded | — | insensitive | 0.088 | 0.112 | 0.112 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 37909 |
| unicode | off | selected_title | FRÜHLING | preferred_title:lower | — | sensitive | 0.123 | 0.129 | 0.159 | 20 | 20 | 20 | 20 | 11 | 10 | 15 | 37909 |
| unicode | off | selected_title | FRÜHLING | preferred_title:lower | — | insensitive | 0.153 | 0.163 | 0.174 | 20 | 20 | 20 | 20 | 11 | 10 | 15 | 37909 |
| unicode | off | filter_name | — | — | name ~ 'Goethe' | sensitive | 0.116 | 0.118 | 0.125 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 37909 |
| unicode | off | filter_name | — | — | name ~ 'Goethe' | insensitive | 0.089 | 0.109 | 0.115 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 37909 |
| unicode | off | filter_lower | — | — | name:lower ~ 'goethe' | sensitive | 0.087 | 0.109 | 0.113 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 37909 |
| unicode | off | filter_lower | — | — | name:lower ~ 'goethe' | insensitive | 0.109 | 0.109 | 0.110 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 37909 |
| unicode | off | filter_folded | — | — | name:folded ~ 'GOETHE' | sensitive | 0.100 | 0.110 | 0.111 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 37909 |
| unicode | off | filter_folded | — | — | name:folded ~ 'GOETHE' | insensitive | 0.110 | 0.112 | 0.112 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 37909 |
| unicode | off | filter_title_lower | — | — | preferred_title:lower ~ 'FRÜHLING' | sensitive | 0.128 | 0.159 | 0.165 | 20 | 20 | 20 | 20 | 11 | 10 | 15 | 37909 |
| unicode | off | filter_title_lower | — | — | preferred_title:lower ~ 'FRÜHLING' | insensitive | 0.113 | 0.142 | 0.163 | 20 | 20 | 20 | 20 | 11 | 10 | 15 | 37909 |
| unicode | off | combined | Goethe | — | name:lower ~ 'goethe' | sensitive | 0.547 | 0.581 | 0.591 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 37909 |
| unicode | off | combined | Goethe | — | name:lower ~ 'goethe' | insensitive | 0.824 | 0.860 | 0.879 | 1 | 1 | 1 | 1 | 1 | 1 | 15 | 37909 |
| unicode | off | exact_posting_control | — | — | name = 'Johann Wolfgang Goethe' | sensitive | 0.053 | 0.081 | 0.089 | 0 | 0 | 0 | 0 | 0 | 0 | 15 | 37909 |
| unicode | off | exact_posting_control | — | — | name = 'Johann Wolfgang Goethe' | insensitive | 0.052 | 0.085 | 0.086 | 0 | 0 | 0 | 0 | 0 | 0 | 15 | 37909 |
| unicode | warm | raw_fts | Der | — | — | sensitive | 0.003 | 0.003 | 0.017 | 0 | 0 | 0 | 0 | 19681 | 10 | 15 | 37909 |
| unicode | warm | raw_fts | Der | — | — | insensitive | 0.003 | 0.003 | 0.016 | 0 | 0 | 0 | 0 | 80871 | 10 | 15 | 37909 |
| unicode | warm | raw_fts | Liebe | — | — | sensitive | 0.003 | 0.004 | 0.015 | 0 | 0 | 0 | 0 | 5589 | 10 | 15 | 37909 |
| unicode | warm | raw_fts | Liebe | — | — | insensitive | 0.005 | 0.006 | 0.020 | 0 | 0 | 0 | 0 | 8017 | 10 | 15 | 37909 |
| unicode | warm | raw_fts | Goethe | — | — | sensitive | 0.006 | 0.008 | 0.028 | 0 | 0 | 0 | 0 | 516 | 10 | 15 | 37909 |
| unicode | warm | raw_fts | Goethe | — | — | insensitive | 0.003 | 0.005 | 0.021 | 0 | 0 | 0 | 0 | 541 | 10 | 15 | 37909 |
| unicode | warm | raw_fts | Müller | — | — | sensitive | 0.004 | 0.005 | 0.031 | 0 | 0 | 0 | 0 | 1164 | 10 | 15 | 37909 |
| unicode | warm | raw_fts | Müller | — | — | insensitive | 0.003 | 0.004 | 0.021 | 0 | 0 | 0 | 0 | 1221 | 10 | 15 | 37909 |
| unicode | warm | raw_fts | Frühling | — | — | sensitive | 0.006 | 0.006 | 0.047 | 0 | 0 | 0 | 0 | 1966 | 10 | 15 | 37909 |
| unicode | warm | raw_fts | Frühling | — | — | insensitive | 0.003 | 0.004 | 0.026 | 0 | 0 | 0 | 0 | 2030 | 10 | 15 | 37909 |
| unicode | warm | raw_fts | ß | — | — | sensitive | 0.004 | 0.004 | 0.017 | 0 | 0 | 0 | 0 | 33078 | 10 | 15 | 37909 |
| unicode | warm | raw_fts | ß | — | — | insensitive | 0.004 | 0.005 | 0.048 | 0 | 0 | 0 | 0 | 55544 | 10 | 15 | 37909 |
| unicode | warm | raw_fts | ss | — | — | sensitive | 0.003 | 0.004 | 0.017 | 0 | 0 | 0 | 0 | 28648 | 10 | 15 | 37909 |
| unicode | warm | raw_fts | ss | — | — | insensitive | 0.004 | 0.004 | 0.015 | 0 | 0 | 0 | 0 | 55544 | 10 | 15 | 37909 |
| unicode | warm | selected_name | Goethe | name | — | sensitive | 0.001 | 0.001 | 0.002 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 37909 |
| unicode | warm | selected_name | Goethe | name | — | insensitive | 0.001 | 0.001 | 0.001 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 37909 |
| unicode | warm | selected_folded | GOETHE | name:folded | — | sensitive | 0.001 | 0.001 | 0.001 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 37909 |
| unicode | warm | selected_folded | GOETHE | name:folded | — | insensitive | 0.001 | 0.001 | 0.001 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 37909 |
| unicode | warm | selected_title | FRÜHLING | preferred_title:lower | — | sensitive | 0.002 | 0.002 | 0.004 | 0 | 0 | 0 | 0 | 11 | 10 | 15 | 37909 |
| unicode | warm | selected_title | FRÜHLING | preferred_title:lower | — | insensitive | 0.002 | 0.003 | 0.004 | 0 | 0 | 0 | 0 | 11 | 10 | 15 | 37909 |
| unicode | warm | filter_name | — | — | name ~ 'Goethe' | sensitive | 0.001 | 0.001 | 0.002 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 37909 |
| unicode | warm | filter_name | — | — | name ~ 'Goethe' | insensitive | 0.001 | 0.001 | 0.001 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 37909 |
| unicode | warm | filter_lower | — | — | name:lower ~ 'goethe' | sensitive | 0.001 | 0.001 | 0.002 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 37909 |
| unicode | warm | filter_lower | — | — | name:lower ~ 'goethe' | insensitive | 0.001 | 0.001 | 0.001 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 37909 |
| unicode | warm | filter_folded | — | — | name:folded ~ 'GOETHE' | sensitive | 0.001 | 0.001 | 0.002 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 37909 |
| unicode | warm | filter_folded | — | — | name:folded ~ 'GOETHE' | insensitive | 0.001 | 0.001 | 0.001 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 37909 |
| unicode | warm | filter_title_lower | — | — | preferred_title:lower ~ 'FRÜHLING' | sensitive | 0.003 | 0.004 | 0.012 | 0 | 0 | 0 | 0 | 11 | 10 | 15 | 37909 |
| unicode | warm | filter_title_lower | — | — | preferred_title:lower ~ 'FRÜHLING' | insensitive | 0.003 | 0.003 | 0.008 | 0 | 0 | 0 | 0 | 11 | 10 | 15 | 37909 |
| unicode | warm | combined | Goethe | — | name:lower ~ 'goethe' | sensitive | 0.002 | 0.002 | 0.009 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 37909 |
| unicode | warm | combined | Goethe | — | name:lower ~ 'goethe' | insensitive | 0.002 | 0.002 | 0.004 | 0 | 0 | 0 | 0 | 1 | 1 | 15 | 37909 |
| unicode | warm | exact_posting_control | — | — | name = 'Johann Wolfgang Goethe' | sensitive | 0.001 | 0.001 | 0.001 | 0 | 0 | 0 | 0 | 0 | 0 | 15 | 37909 |
| unicode | warm | exact_posting_control | — | — | name = 'Johann Wolfgang Goethe' | insensitive | 0.001 | 0.001 | 0.001 | 0 | 0 | 0 | 0 | 0 | 0 | 15 | 37909 |
