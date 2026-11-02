# 50 multilingual search queries: ASCII versus UTF-8 byte bigrams — 2 November 2026

## Corpus and method

The corpus contains 400,000 JSON records in one schema-free `texts` collection: the first
50,000 sentences from each of eight Leipzig Corpora Collection 2020 news archives. Languages
are Arabic, German, French, Japanese, Polish, Russian, Spanish, and Chinese. The source sentences
are real text; `prepare_leipzig_corpus.py` generates the JSON files. The corpus was held on
Linux `/tmp` (tmpfs); its JSON files occupy 1.6 GiB of tmpfs allocation. The archives and
generated corpus are not committed. Hardware: Intel Core i9-13900K.

Each of the [50 filters](workloads/leipzig-multilingual-50.json) ran with the Bigram JSON backend,
`count=true`, one returned record, and the repository’s 10,000-result cap. Every index was
warmed before an untimed pass. Each process then ran seven timed rounds in alternating query
order. Three processes were run per encoding with alternating encoding order. Table values are
medians of the three within-process medians. Index construction, process startup, and the
untimed pass are excluded from query timings.

All 50 capped counts and first result IDs matched across all six processes. Both modes
built indexes from source in every process (zero snapshot loads).

| Measure | ASCII | UTF-8 bytes |
|---|---:|---:|
| Sum of 50 query medians | 5333.7 ms | 407.4 ms |
| Median query | 40.30 ms | 5.28 ms |
| Source reads across 50 queries | 9,963,490 | 258,049 |
| Faster cases | 7 | 43 |
| Median index warm time (excluded) | 691.3 ms | 1637.4 ms |

The summed search time was 13.1× lower with UTF-8 byte postings. Source reads
fell 38.6×. Pure Arabic, Cyrillic, Japanese, and Chinese terms cause the ASCII
index to admit nearly every file; UTF-8 byte pairs narrow them. A few already selective ASCII
or Latin terms are similar or slower in UTF-8 mode, as the per-query table shows.

| # | Language | Filter | Matches | ASCII ms | UTF-8 ms | Speedup | Source reads ASCII → UTF-8 |
|---:|---|---|---:|---:|---:|---:|---:|
| 1 | Arabic | `text ~ 'كورونا'` | 4,368 | 205.33 | 17.58 | 11.7× | 400,001 → 16,833 |
| 2 | Arabic | `text ~ 'فيروس'` | 3,734 | 204.40 | 21.50 | 9.5× | 400,001 → 23,970 |
| 3 | Arabic | `text ~ 'الحكومة'` | 1,399 | 204.25 | 11.98 | 17.1× | 400,001 → 11,893 |
| 4 | Arabic | `text ~ 'لبنان'` | 1,776 | 203.12 | 26.85 | 7.6× | 400,001 → 26,977 |
| 5 | Arabic | `text ~ 'الأمريكية'` | 868 | 203.69 | 13.79 | 14.8× | 400,001 → 12,727 |
| 6 | Arabic | `text ~ 'برلين'` | 107 | 206.46 | 27.38 | 7.5× | 400,001 → 24,908 |
| 7 | German | `text ~ 'über'` | 3,549 | 24.65 | 5.45 | 4.5× | 16,688 → 4,047 |
| 8 | German | `text ~ 'können'` | 973 | 16.09 | 3.63 | 4.4× | 1,750 → 1,009 |
| 9 | German | `text ~ 'müssen'` | 541 | 10.13 | 3.52 | 2.9× | 3,067 → 577 |
| 10 | German | `text ~ 'dafür'` | 365 | 6.74 | 3.03 | 2.2× | 609 → 577 |
| 11 | German | `text ~ 'möglich'` | 806 | 4.38 | 5.01 | 0.9× | 1,288 → 979 |
| 12 | German folded | `text ~ :ignorecase('ÜBER')` | 3,972 | 39.06 | 10.12 | 3.9× | 39,232 → 4,201 |
| 13 | French | `text ~ 'être'` | 2,052 | 19.58 | 5.30 | 3.7× | 19,864 → 2,501 |
| 14 | French | `text ~ 'après'` | 1,329 | 12.46 | 5.97 | 2.1× | 3,747 → 2,135 |
| 15 | French | `text ~ 'président'` | 1,064 | 6.11 | 6.15 | 1.0× | 3,482 → 1,348 |
| 16 | French | `text ~ 'pandémie'` | 543 | 6.62 | 5.07 | 1.3× | 1,131 → 546 |
| 17 | French | `text ~ 'première'` | 665 | 7.76 | 4.65 | 1.7× | 1,566 → 727 |
| 18 | French folded | `text ~ :ignorecase('ÊTRE')` | 2,060 | 27.75 | 6.75 | 4.1× | 25,757 → 2,614 |
| 19 | Japanese | `text ~ 'コロナ'` | 1,499 | 206.30 | 4.16 | 49.6× | 400,001 → 1,561 |
| 20 | Japanese | `text ~ '感染'` | 1,999 | 204.47 | 5.05 | 40.5× | 400,001 → 2,008 |
| 21 | Japanese | `text ~ 'ゲーム'` | 951 | 205.05 | 3.50 | 58.6× | 400,001 → 1,011 |
| 22 | Japanese | `text ~ '東京'` | 378 | 205.07 | 2.47 | 83.0× | 400,001 → 380 |
| 23 | Japanese | `text ~ '政府'` | 1,890 | 204.78 | 5.48 | 37.4× | 400,001 → 1,897 |
| 24 | Japanese | `text ~ '東京都'` | 115 | 204.84 | 2.16 | 94.6× | 400,001 → 119 |
| 25 | Polish | `text ~ 'będzie'` | 1,859 | 11.24 | 7.93 | 1.4× | 4,948 → 2,172 |
| 26 | Polish | `text ~ 'które'` | 2,020 | 8.23 | 8.77 | 0.9× | 6,118 → 2,625 |
| 27 | Polish | `text ~ 'może'` | 1,324 | 41.29 | 6.73 | 6.1× | 39,281 → 2,546 |
| 28 | Polish | `text ~ 'także'` | 997 | 8.68 | 5.04 | 1.7× | 4,126 → 1,041 |
| 29 | Polish | `text ~ 'również'` | 657 | 4.43 | 4.61 | 1.0× | 1,186 → 673 |
| 30 | Polish folded | `text ~ :ignorecase('BĘDZIE')` | 2,056 | 11.82 | 6.56 | 1.8× | 15,287 → 2,435 |
| 31 | Russian | `text ~ 'время'` | 1,661 | 208.23 | 14.25 | 14.6× | 400,001 → 6,543 |
| 32 | Russian | `text ~ 'России'` | 1,162 | 206.09 | 5.33 | 38.7× | 400,001 → 3,058 |
| 33 | Russian | `text ~ 'человек'` | 1,447 | 204.32 | 32.10 | 6.4× | 400,001 → 19,235 |
| 34 | Russian | `text ~ 'коронавируса'` | 725 | 206.12 | 12.95 | 15.9× | 400,001 → 10,338 |
| 35 | Russian | `text ~ 'лечение'` | 134 | 205.70 | 17.43 | 11.8× | 400,001 → 18,669 |
| 36 | Russian folded | `text ~ :ignorecase('РОССИИ')` | 1,167 | 218.25 | 28.86 | 7.6× | 400,001 → 28,322 |
| 37 | Spanish | `text ~ 'también'` | 1,936 | 5.16 | 4.56 | 1.1× | 1,948 → 1,938 |
| 38 | Spanish | `text ~ 'años'` | 2,060 | 36.58 | 5.41 | 6.8× | 56,963 → 3,303 |
| 39 | Spanish | `text ~ 'país'` | 1,879 | 36.68 | 3.60 | 10.2× | 52,389 → 2,033 |
| 40 | Spanish | `text ~ 'situación'` | 725 | 4.07 | 5.26 | 0.8× | 821 → 757 |
| 41 | Spanish | `text ~ 'México'` | 471 | 3.36 | 3.00 | 1.1× | 506 → 478 |
| 42 | Spanish folded | `text ~ :ignorecase('PAÍS')` | 1,923 | 39.31 | 4.24 | 9.3× | 60,542 → 2,036 |
| 43 | Chinese | `text ~ '习近平'` | 827 | 204.04 | 2.31 | 88.3× | 400,001 → 828 |
| 44 | Chinese | `text ~ '总书记'` | 633 | 204.29 | 2.13 | 96.0× | 400,001 → 636 |
| 45 | Chinese | `text ~ '加拿大'` | 587 | 205.26 | 2.34 | 87.8× | 400,001 → 588 |
| 46 | Chinese | `text ~ '特朗普'` | 574 | 204.38 | 1.91 | 107.2× | 400,001 → 575 |
| 47 | Chinese | `text ~ '年轻人'` | 99 | 204.46 | 1.89 | 108.1× | 400,001 → 122 |
| 48 | Chinese | `text ~ '不可能'` | 136 | 205.36 | 2.05 | 100.2× | 400,001 → 383 |
| 49 | ASCII control | `text ~ 'Berlin'` | 666 | 2.66 | 2.83 | 0.9× | 723 → 723 |
| 50 | ASCII control | `text ~ 'Paris'` | 393 | 4.61 | 8.75 | 0.5× | 447 → 447 |

## Source archives

Each archive is fetched from the [Leipzig Corpora Collection](https://wortschatz.uni-leipzig.de/en/download).
The SHA-256 values pin the exact inputs used here.

| Language code | Archive | SHA-256 |
|---|---|---|
| ara | [ara_news_2020_100K.tar.gz](https://downloads.wortschatz-leipzig.de/corpora/ara_news_2020_100K.tar.gz) | `ad4eaa54154c4cf4e9ec0f5da42e0528fcd35badcc40fd31f6c0c39a68f3f37d` |
| deu | [deu_news_2020_100K.tar.gz](https://downloads.wortschatz-leipzig.de/corpora/deu_news_2020_100K.tar.gz) | `20d96f9e3686ff236b9e40e5d92fca8369479aa4eec01397d16a9036bac765c6` |
| fra | [fra_news_2020_100K.tar.gz](https://downloads.wortschatz-leipzig.de/corpora/fra_news_2020_100K.tar.gz) | `0013db37d9081956e910769a29901bd3a3c34fca3051fc2839cd9b85e73ea133` |
| jpn | [jpn_news_2020_100K.tar.gz](https://downloads.wortschatz-leipzig.de/corpora/jpn_news_2020_100K.tar.gz) | `5fbaa0f02f0d9bebcb60b3a481e2c49e01d08b136bae5f77496273a4b7fbc683` |
| pol | [pol_news_2020_100K.tar.gz](https://downloads.wortschatz-leipzig.de/corpora/pol_news_2020_100K.tar.gz) | `156572f69e8cad38901679010f59ca596784feca86c97940febba9f8538e869a` |
| rus | [rus_news_2020_100K.tar.gz](https://downloads.wortschatz-leipzig.de/corpora/rus_news_2020_100K.tar.gz) | `2f78e5a68873ee586b29a36f6daa45ec8ab90edda6defc9b2911a4857b211b7e` |
| spa | [spa_news_2020_100K.tar.gz](https://downloads.wortschatz-leipzig.de/corpora/spa_news_2020_100K.tar.gz) | `96323c33f61720dce3f15d41c7e3d375d19de2acd1eebf106cb28dc476360217` |
| zho | [zho_news_2020_100K.tar.gz](https://downloads.wortschatz-leipzig.de/corpora/zho_news_2020_100K.tar.gz) | `ba219ac3a5b9e13f2653690ab67a03a184cab6019cbbb374791fd4cc7446a7e1` |

## Reproduce

```sh
python3 crates/bench/prepare_leipzig_corpus.py /tmp/objekte-leipzig-2020 --per-language 50000
cargo build --release -p objekte-bench --bin json-query-mix
target/release/json-query-mix --repo /tmp/objekte-leipzig-2020 --workload crates/bench/workloads/leipzig-multilingual-50.json --no-schema --runs 7 --bigram-encoding ascii
target/release/json-query-mix --repo /tmp/objekte-leipzig-2020 --workload crates/bench/workloads/leipzig-multilingual-50.json --no-schema --runs 7 --bigram-encoding utf8_bytes
```

The six raw runs are:

- ascii, process 1: [NDJSON](results/leipzig-400k-mix-50-ascii-2026-11-02.ndjson); summed medians 5324.8 ms; index warm 696.1 ms.
- ascii, process 2: [NDJSON](results/leipzig-400k-mix-50-ascii-2026-11-02-repeat2.ndjson); summed medians 5404.0 ms; index warm 685.1 ms.
- ascii, process 3: [NDJSON](results/leipzig-400k-mix-50-ascii-2026-11-02-repeat3.ndjson); summed medians 5307.9 ms; index warm 691.3 ms.
- utf8_bytes, process 1: [NDJSON](results/leipzig-400k-mix-50-utf8-bytes-2026-11-02.ndjson); summed medians 389.5 ms; index warm 1645.0 ms.
- utf8_bytes, process 2: [NDJSON](results/leipzig-400k-mix-50-utf8-bytes-2026-11-02-repeat2.ndjson); summed medians 408.2 ms; index warm 1627.4 ms.
- utf8_bytes, process 3: [NDJSON](results/leipzig-400k-mix-50-utf8-bytes-2026-11-02-repeat3.ndjson); summed medians 422.1 ms; index warm 1637.4 ms.
