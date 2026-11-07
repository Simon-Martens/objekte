# Query extension feature workload

Measured core and harness: `6b49eb26b03117930b8c47b7885b126988dda21e`. Fixed nine-record, two-relation fixture; four profiles and five repetitions per case. Each case was checked against an independently specified expected result and the reference route before timing. These are first measurements with no historical equivalent. The microsecond-scale samples are descriptive, not a speedup claim.

The raw JSONL files contain exact filter/FTS/options/parameters, plans, counters and all samples. The table shows median search time and counters from the median sample. Fixture creation, build, reference answer checks and EXPLAIN are outside timed search calls.

| Case | Alphabet | Cache | Median ms | Min–max ms | Candidates | Scanned | Source opens | Target reads | Edge visits | Matches | Page |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| parameter-a | ascii | off | 0.002650 | 0.002435–0.004656 | 2 | 0 | 0 | 0 | 0 | 2 | 2 |
| parameter-b | ascii | off | 0.002300 | 0.001984–0.002762 | 1 | 0 | 0 | 0 | 0 | 1 | 1 |
| position | ascii | off | 0.019718 | 0.019247–0.021985 | 6 | 6 | 9 | 3 | 4 | 2 | 2 |
| field-comparison | ascii | off | 0.020850 | 0.019915–0.025385 | 6 | 6 | 10 | 4 | 5 | 2 | 2 |
| sort-only | ascii | off | 0.002272 | 0.002106–0.002986 | 6 | 0 | 0 | 0 | 0 | 6 | 6 |
| sort-deep-page | ascii | off | 0.002440 | 0.002305–0.007929 | 6 | 0 | 0 | 0 | 0 | 6 | 2 |
| sort-parameter | ascii | off | 0.003015 | 0.002864–0.004693 | 2 | 0 | 0 | 0 | 0 | 2 | 2 |
| sort-multi-key | ascii | off | 0.001455 | 0.001316–0.001953 | 3 | 0 | 0 | 0 | 0 | 3 | 3 |
| temporal | ascii | off | 0.002398 | 0.002255–0.003264 | 2 | 0 | 0 | 0 | 0 | 2 | 2 |
| temporal-sort | ascii | off | 0.002756 | 0.002639–0.003323 | 2 | 0 | 0 | 0 | 0 | 2 | 2 |
| byte-size | ascii | off | 0.002113 | 0.001946–0.002508 | 1 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-useful | ascii | off | 0.003399 | 0.003306–0.004509 | 1 | 1 | 1 | 0 | 0 | 1 | 1 |
| regex-unrestricted | ascii | off | 0.007136 | 0.006836–0.008244 | 3 | 3 | 3 | 0 | 0 | 3 | 3 |
| relation-target-first | ascii | off | 0.005012 | 0.004802–0.006617 | 2 | 0 | 0 | 0 | 0 | 2 | 2 |
| relation-source-restricted | ascii | off | 0.007206 | 0.006754–0.008665 | 1 | 0 | 0 | 0 | 1 | 1 | 1 |
| relation-any | ascii | off | 0.004363 | 0.004294–0.005608 | 2 | 0 | 0 | 0 | 0 | 2 | 2 |
| selected-fts-position | ascii | off | 0.025056 | 0.023676–0.026256 | 6 | 6 | 9 | 3 | 4 | 2 | 2 |
| relation-dangling-not | ascii | off | 0.020036 | 0.019619–0.021277 | 6 | 6 | 10 | 4 | 5 | 3 | 3 |
| backlinks | ascii | off | 0.012683 | 0.012424–0.017412 | 3 | 3 | 6 | 3 | 3 | 1 | 1 |
| regex-parameter-relation | ascii | off | 0.009471 | 0.008516–0.011990 | 1 | 0 | 1 | 1 | 0 | 1 | 1 |
| regex-parameter-relation-sort | ascii | off | 0.006643 | 0.006339–0.007950 | 2 | 0 | 1 | 1 | 0 | 2 | 2 |
| parameter-a | ascii | warm | 0.001001 | 0.000931–0.001915 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| parameter-b | ascii | warm | 0.000932 | 0.000897–0.001191 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| position | ascii | warm | 0.001178 | 0.001116–0.001895 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| field-comparison | ascii | warm | 0.001585 | 0.001350–0.005014 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| sort-only | ascii | warm | 0.000840 | 0.000706–0.001243 | 0 | 0 | 0 | 0 | 0 | 6 | 6 |
| sort-deep-page | ascii | warm | 0.000527 | 0.000513–0.000608 | 0 | 0 | 0 | 0 | 0 | 6 | 2 |
| sort-parameter | ascii | warm | 0.001177 | 0.001156–0.001433 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| sort-multi-key | ascii | warm | 0.000754 | 0.000701–0.001127 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| temporal | ascii | warm | 0.000852 | 0.000837–0.001088 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| temporal-sort | ascii | warm | 0.001137 | 0.001050–0.001249 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| byte-size | ascii | warm | 0.000690 | 0.000671–0.000871 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-useful | ascii | warm | 0.000581 | 0.000545–0.001331 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-unrestricted | ascii | warm | 0.000651 | 0.000591–0.000953 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| relation-target-first | ascii | warm | 0.001100 | 0.001030–0.001532 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| relation-source-restricted | ascii | warm | 0.002052 | 0.001816–0.002760 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| relation-any | ascii | warm | 0.001125 | 0.001101–0.001825 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| selected-fts-position | ascii | warm | 0.000944 | 0.000820–0.001138 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| relation-dangling-not | ascii | warm | 0.001251 | 0.001158–0.001713 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| backlinks | ascii | warm | 0.001217 | 0.001134–0.001730 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-parameter-relation | ascii | warm | 0.002209 | 0.002031–0.003790 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-parameter-relation-sort | ascii | warm | 0.001691 | 0.001667–0.002497 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| parameter-a | unicode | off | 0.002686 | 0.002594–0.004078 | 2 | 0 | 0 | 0 | 0 | 2 | 2 |
| parameter-b | unicode | off | 0.002279 | 0.002098–0.002972 | 1 | 0 | 0 | 0 | 0 | 1 | 1 |
| position | unicode | off | 0.020191 | 0.019548–0.021809 | 6 | 6 | 9 | 3 | 4 | 2 | 2 |
| field-comparison | unicode | off | 0.021032 | 0.020897–0.022269 | 6 | 6 | 10 | 4 | 5 | 2 | 2 |
| sort-only | unicode | off | 0.002401 | 0.002243–0.002740 | 6 | 0 | 0 | 0 | 0 | 6 | 6 |
| sort-deep-page | unicode | off | 0.002151 | 0.001989–0.006430 | 6 | 0 | 0 | 0 | 0 | 6 | 2 |
| sort-parameter | unicode | off | 0.003047 | 0.002737–0.003382 | 2 | 0 | 0 | 0 | 0 | 2 | 2 |
| sort-multi-key | unicode | off | 0.001362 | 0.001313–0.001922 | 3 | 0 | 0 | 0 | 0 | 3 | 3 |
| temporal | unicode | off | 0.002514 | 0.002358–0.003889 | 2 | 0 | 0 | 0 | 0 | 2 | 2 |
| temporal-sort | unicode | off | 0.002731 | 0.002718–0.003173 | 2 | 0 | 0 | 0 | 0 | 2 | 2 |
| byte-size | unicode | off | 0.002060 | 0.001948–0.002425 | 1 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-useful | unicode | off | 0.003477 | 0.003359–0.004440 | 1 | 1 | 1 | 0 | 0 | 1 | 1 |
| regex-unrestricted | unicode | off | 0.007140 | 0.006887–0.007980 | 3 | 3 | 3 | 0 | 0 | 3 | 3 |
| relation-target-first | unicode | off | 0.004929 | 0.004417–0.006834 | 2 | 0 | 0 | 0 | 0 | 2 | 2 |
| relation-source-restricted | unicode | off | 0.007463 | 0.007401–0.009245 | 1 | 0 | 0 | 0 | 1 | 1 | 1 |
| relation-any | unicode | off | 0.005154 | 0.004965–0.006232 | 2 | 0 | 0 | 0 | 0 | 2 | 2 |
| selected-fts-position | unicode | off | 0.025491 | 0.024617–0.027341 | 6 | 6 | 9 | 3 | 4 | 2 | 2 |
| relation-dangling-not | unicode | off | 0.020854 | 0.019868–0.022752 | 6 | 6 | 10 | 4 | 5 | 3 | 3 |
| backlinks | unicode | off | 0.012532 | 0.012379–0.018187 | 3 | 3 | 6 | 3 | 3 | 1 | 1 |
| regex-parameter-relation | unicode | off | 0.009539 | 0.009149–0.011831 | 1 | 0 | 1 | 1 | 0 | 1 | 1 |
| regex-parameter-relation-sort | unicode | off | 0.006966 | 0.006558–0.008023 | 2 | 0 | 1 | 1 | 0 | 2 | 2 |
| parameter-a | unicode | warm | 0.001005 | 0.000881–0.003028 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| parameter-b | unicode | warm | 0.000958 | 0.000861–0.001070 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| position | unicode | warm | 0.001157 | 0.001103–0.002259 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| field-comparison | unicode | warm | 0.001463 | 0.001416–0.002379 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| sort-only | unicode | warm | 0.000886 | 0.000725–0.001357 | 0 | 0 | 0 | 0 | 0 | 6 | 6 |
| sort-deep-page | unicode | warm | 0.000522 | 0.000479–0.000668 | 0 | 0 | 0 | 0 | 0 | 6 | 2 |
| sort-parameter | unicode | warm | 0.001226 | 0.001115–0.001288 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| sort-multi-key | unicode | warm | 0.000738 | 0.000658–0.000938 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| temporal | unicode | warm | 0.000885 | 0.000845–0.001194 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| temporal-sort | unicode | warm | 0.001056 | 0.000996–0.001159 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| byte-size | unicode | warm | 0.000638 | 0.000599–0.000876 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-useful | unicode | warm | 0.000557 | 0.000537–0.001275 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-unrestricted | unicode | warm | 0.000633 | 0.000590–0.000959 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| relation-target-first | unicode | warm | 0.001073 | 0.001032–0.001521 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| relation-source-restricted | unicode | warm | 0.001885 | 0.001753–0.002437 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| relation-any | unicode | warm | 0.001106 | 0.001065–0.001535 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| selected-fts-position | unicode | warm | 0.000972 | 0.000807–0.001116 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| relation-dangling-not | unicode | warm | 0.001158 | 0.001106–0.001655 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| backlinks | unicode | warm | 0.001288 | 0.001078–0.001642 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-parameter-relation | unicode | warm | 0.002148 | 0.001990–0.003350 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-parameter-relation-sort | unicode | warm | 0.001585 | 0.001485–0.010223 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |

## Fixture construction

| Alphabet | Cache | Wall ms | Relation count | Edges | Resident edge bytes |
| --- | --- | ---: | ---: | ---: | ---: |
| ascii | off | 1.981 | 2 | 11 | 736 |
| ascii | warm | 1.810 | 2 | 11 | 736 |
| unicode | off | 1.584 | 2 | 11 | 736 |
| unicode | warm | 1.424 | 2 | 11 | 736 |
