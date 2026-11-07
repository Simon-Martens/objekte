# Development feature workload

Core revision: `21077340f965f72b234b87b7667d7451cbed0e3e`. 21 independently specified cases, 420 timed samples, four profiles. Each case passed expected-answer and reference-route checks before timing.

The nine-record fixture has two relations; the build rows retain edge count and resident edge bytes. Times below are medians in milliseconds; the raw JSONL files retain every query, parameter, plan, counter and sample. Microsecond differences are descriptive.

| Case | Alphabet | Cache | Median ms | Min–max ms | Candidates | Scanned | Opens | Target reads | Reused target values | Backlink probes | Matches | Page |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| parameter-a | ascii | off | 0.006599 | 0.006363–0.016211 | 2 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| parameter-b | ascii | off | 0.005910 | 0.005381–0.008304 | 1 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| position | ascii | off | 0.053324 | 0.051255–0.061050 | 6 | 6 | 9 | 3 | 0 | 0 | 2 | 2 |
| field-comparison | ascii | off | 0.049677 | 0.048998–0.053995 | 6 | 6 | 9 | 3 | 4 | 0 | 2 | 2 |
| sort-only | ascii | off | 0.004573 | 0.004228–0.005180 | 6 | 0 | 0 | 0 | 0 | 0 | 6 | 6 |
| sort-deep-page | ascii | off | 0.004479 | 0.004192–0.004942 | 6 | 0 | 0 | 0 | 0 | 0 | 6 | 2 |
| sort-parameter | ascii | off | 0.007068 | 0.006624–0.008454 | 2 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| sort-multi-key | ascii | off | 0.003378 | 0.003110–0.004076 | 3 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| temporal | ascii | off | 0.006070 | 0.005494–0.007334 | 2 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| temporal-sort | ascii | off | 0.006782 | 0.006376–0.008370 | 2 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| byte-size | ascii | off | 0.005599 | 0.005327–0.006191 | 1 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-useful | ascii | off | 0.010616 | 0.009648–0.013081 | 1 | 1 | 1 | 0 | 0 | 0 | 1 | 1 |
| regex-unrestricted | ascii | off | 0.018736 | 0.018183–0.021728 | 3 | 3 | 3 | 0 | 0 | 0 | 3 | 3 |
| relation-target-first | ascii | off | 0.011448 | 0.011019–0.013404 | 2 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| relation-source-restricted | ascii | off | 0.015924 | 0.015486–0.018741 | 1 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| relation-any | ascii | off | 0.011230 | 0.010730–0.012566 | 2 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| selected-fts-position | ascii | off | 0.059064 | 0.056296–0.061329 | 6 | 6 | 9 | 3 | 0 | 0 | 2 | 2 |
| relation-dangling-not | ascii | off | 0.034047 | 0.020778–0.072015 | 6 | 6 | 10 | 4 | 0 | 0 | 3 | 3 |
| backlinks | ascii | off | 0.013243 | 0.012949–0.014881 | 3 | 3 | 6 | 3 | 0 | 3 | 1 | 1 |
| regex-parameter-relation | ascii | off | 0.014236 | 0.009284–0.015072 | 1 | 0 | 1 | 1 | 0 | 0 | 1 | 1 |
| regex-parameter-relation-sort | ascii | off | 0.006622 | 0.006496–0.008187 | 2 | 0 | 1 | 1 | 1 | 0 | 2 | 2 |
| parameter-a | ascii | warm | 0.001348 | 0.000971–0.002706 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| parameter-b | ascii | warm | 0.000970 | 0.000824–0.001173 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| position | ascii | warm | 0.001253 | 0.001104–0.002077 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| field-comparison | ascii | warm | 0.001619 | 0.001439–0.002710 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| sort-only | ascii | warm | 0.000820 | 0.000725–0.001213 | 0 | 0 | 0 | 0 | 0 | 0 | 6 | 6 |
| sort-deep-page | ascii | warm | 0.000565 | 0.000525–0.000593 | 0 | 0 | 0 | 0 | 0 | 0 | 6 | 2 |
| sort-parameter | ascii | warm | 0.001366 | 0.001126–0.001734 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| sort-multi-key | ascii | warm | 0.000711 | 0.000666–0.000974 | 0 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| temporal | ascii | warm | 0.000865 | 0.000788–0.001205 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| temporal-sort | ascii | warm | 0.001146 | 0.001007–0.001156 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| byte-size | ascii | warm | 0.000690 | 0.000660–0.001062 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-useful | ascii | warm | 0.000612 | 0.000509–0.001279 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-unrestricted | ascii | warm | 0.000681 | 0.000630–0.001001 | 0 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| relation-target-first | ascii | warm | 0.001222 | 0.001018–0.001947 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| relation-source-restricted | ascii | warm | 0.001996 | 0.001877–0.002768 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| relation-any | ascii | warm | 0.001076 | 0.001063–0.001646 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| selected-fts-position | ascii | warm | 0.000883 | 0.000791–0.001297 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| relation-dangling-not | ascii | warm | 0.001289 | 0.001142–0.001895 | 0 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| backlinks | ascii | warm | 0.001180 | 0.001034–0.001499 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-parameter-relation | ascii | warm | 0.002285 | 0.001847–0.006571 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-parameter-relation-sort | ascii | warm | 0.001859 | 0.001695–0.002881 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| parameter-a | unicode | off | 0.002706 | 0.002502–0.004629 | 2 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| parameter-b | unicode | off | 0.002218 | 0.002023–0.003441 | 1 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| position | unicode | off | 0.019679 | 0.018766–0.021895 | 6 | 6 | 9 | 3 | 0 | 0 | 2 | 2 |
| field-comparison | unicode | off | 0.018759 | 0.018029–0.020213 | 6 | 6 | 9 | 3 | 4 | 0 | 2 | 2 |
| sort-only | unicode | off | 0.002014 | 0.001746–0.002869 | 6 | 0 | 0 | 0 | 0 | 0 | 6 | 6 |
| sort-deep-page | unicode | off | 0.002294 | 0.002133–0.002629 | 6 | 0 | 0 | 0 | 0 | 0 | 6 | 2 |
| sort-parameter | unicode | off | 0.003042 | 0.002905–0.003787 | 2 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| sort-multi-key | unicode | off | 0.001291 | 0.001245–0.001737 | 3 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| temporal | unicode | off | 0.002567 | 0.002425–0.003202 | 2 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| temporal-sort | unicode | off | 0.002825 | 0.002677–0.003216 | 2 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| byte-size | unicode | off | 0.002189 | 0.002046–0.002617 | 1 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-useful | unicode | off | 0.003481 | 0.003266–0.005016 | 1 | 1 | 1 | 0 | 0 | 0 | 1 | 1 |
| regex-unrestricted | unicode | off | 0.007445 | 0.007224–0.008199 | 3 | 3 | 3 | 0 | 0 | 0 | 3 | 3 |
| relation-target-first | unicode | off | 0.005295 | 0.004795–0.006796 | 2 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| relation-source-restricted | unicode | off | 0.010359 | 0.007567–0.011973 | 1 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| relation-any | unicode | off | 0.005102 | 0.004775–0.006368 | 2 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| selected-fts-position | unicode | off | 0.025922 | 0.024844–0.029827 | 6 | 6 | 9 | 3 | 0 | 0 | 2 | 2 |
| relation-dangling-not | unicode | off | 0.022311 | 0.020832–0.022450 | 6 | 6 | 10 | 4 | 0 | 0 | 3 | 3 |
| backlinks | unicode | off | 0.013562 | 0.013080–0.015945 | 3 | 3 | 6 | 3 | 0 | 3 | 1 | 1 |
| regex-parameter-relation | unicode | off | 0.009091 | 0.008938–0.012608 | 1 | 0 | 1 | 1 | 0 | 0 | 1 | 1 |
| regex-parameter-relation-sort | unicode | off | 0.006865 | 0.006292–0.010827 | 2 | 0 | 1 | 1 | 1 | 0 | 2 | 2 |
| parameter-a | unicode | warm | 0.001365 | 0.000936–0.003753 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| parameter-b | unicode | warm | 0.001116 | 0.000950–0.001966 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| position | unicode | warm | 0.001215 | 0.001114–0.002535 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| field-comparison | unicode | warm | 0.001617 | 0.001462–0.002752 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| sort-only | unicode | warm | 0.000748 | 0.000714–0.001326 | 0 | 0 | 0 | 0 | 0 | 0 | 6 | 6 |
| sort-deep-page | unicode | warm | 0.000523 | 0.000498–0.000624 | 0 | 0 | 0 | 0 | 0 | 0 | 6 | 2 |
| sort-parameter | unicode | warm | 0.001226 | 0.001068–0.001970 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| sort-multi-key | unicode | warm | 0.001021 | 0.000827–0.001812 | 0 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| temporal | unicode | warm | 0.001011 | 0.000903–0.001832 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| temporal-sort | unicode | warm | 0.002168 | 0.001337–0.002700 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| byte-size | unicode | warm | 0.000844 | 0.000724–0.001537 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-useful | unicode | warm | 0.000618 | 0.000519–0.001743 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-unrestricted | unicode | warm | 0.000752 | 0.000703–0.001084 | 0 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| relation-target-first | unicode | warm | 0.001403 | 0.001140–0.002251 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| relation-source-restricted | unicode | warm | 0.002172 | 0.001879–0.002995 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| relation-any | unicode | warm | 0.001269 | 0.001110–0.001983 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| selected-fts-position | unicode | warm | 0.000995 | 0.000903–0.001239 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |
| relation-dangling-not | unicode | warm | 0.001256 | 0.001197–0.009208 | 0 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |
| backlinks | unicode | warm | 0.001341 | 0.001170–0.002444 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-parameter-relation | unicode | warm | 0.002216 | 0.002017–0.003801 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 1 |
| regex-parameter-relation-sort | unicode | warm | 0.001645 | 0.001557–0.003009 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 2 |

## Construction

| Alphabet | Cache | Build ms | Relations | Edges | Edge bytes |
| --- | --- | ---: | ---: | ---: | ---: |
| ascii | off | 1.791 | 2 | 11 | 736 |
| ascii | warm | 1.584 | 2 | 11 | 736 |
| unicode | off | 2.102 | 2 | 11 | 736 |
| unicode | warm | 2.452 | 2 | 11 | 736 |
