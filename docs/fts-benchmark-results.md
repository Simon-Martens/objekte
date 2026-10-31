# Full-text benchmark on Musenalm

Measured 2026-10-31 on a 13th Gen Intel Core i9-13900K (32 logical CPUs), Linux x86-64, release build. At the time, the `contents` collection had 133,948 JSON files (524 MiB on disk). The checked-in files now also contain an `entry` reference, so these build, memory, and query figures describe the earlier fixture. Every JSON leaf value was included in the FTS index. A temporary schema overlay assigned `annotation` = Markdown, `preferred_title` = plain, `title_statement` = HTML, and `contributors[].annotation` = XML; it did not edit the example repository.

Run with `cargo run --release -p objekte-bench --bin fts-query-mix -- examples/musenalm 3`. Build time was measured once in each of three fresh processes. Ready RSS was sampled 250 ms after each build. Query results below are medians of three warm timed runs after one warm-up pass per case; each query counts all matches and returns one record.

## Index build and memory

| Run | Base build ms | FTS build ms | RSS after base MiB | RSS after FTS MiB | FTS RSS delta MiB | Build peak RSS MiB |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 559.8 | 258.5 | 205.4 | 241.1 | 35.7 | 367.0 |
| 2 | 564.7 | 260.3 | 205.7 | 240.8 | 35.2 | 366.1 |
| 3 | 550.5 | 259.1 | 184.6 | 218.6 | 34.0 | 344.4 |

Compressed base bigram postings: 16,093,219 bytes (15.35 MiB). Compressed FTS postings: 32,165,317 bytes (30.68 MiB). Combined compressed postings: 48,258,536 bytes (46.02 MiB). These posting sizes exclude file slots, typed indexes, allocator overhead, and temporary build memory; ready RSS includes them.

### Shared source pass follow-up

After changing markup-declared collections to build both indexes from the same file read and Sonic parse, three fresh release processes gave the following results. Each process ran one timed pass over the 30 queries; the table records only build and memory measurements. The initial build includes both indexes, and the explicit `warm_text_index` call afterward is a no-op. The schema overlay and source files match the measurements above.

| Run | Combined build ms | Explicit FTS warm ms | Ready RSS MiB | Build peak RSS MiB |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 777.2 | 0.1 | 222.4 | 431.6 |
| 2 | 787.2 | 0.1 | 208.9 | 430.6 |
| 3 | 783.5 | 0.1 | 239.4 | 434.9 |

The previous two-pass builds totaled 818.3, 825.0, and 809.6 ms. The shared pass saved roughly 20–50 ms in this unpaired comparison while raising peak build RSS by roughly 65–90 MiB. Compressed posting sizes stayed identical, and all 30 query match counts matched the earlier shared-pass run. Ready RSS varies because allocator pages are not returned uniformly after the build.

## Counted queries

`FTS cand.` counts files selected by the FTS bigram lookup before intersection with other JSON indexes. `Scans` counts files checked against source. `Parses` includes the extra parse of the returned page record. A zero in `FTS cand.` can also mean no safe FTS literal was available, so another planner path or a full scan was used.

| # | Mode | Query / field | Matches | FTS cand. | Scans | Parses | Median ms | Min–max ms |
| ---: | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | filter | `annotation:fts ~ 'Goet'` | 293 | 611 | 611 | 612 | 10.26 | 9.98–10.83 |
| 2 | filter | `annotation:fts ~ 'Schill'` | 747 | 2,328 | 2,328 | 2,329 | 15.03 | 14.31–18.14 |
| 3 | filter | `annotation:fts ~ 'Mond'` | 45 | 5,216 | 5,216 | 5,217 | 18.38 | 17.80–19.55 |
| 4 | filter | `annotation:fts ~ :ignorecase('GOET')` | 293 | 611 | 611 | 612 | 10.92 | 10.36–11.03 |
| 5 | filter | `annotation:fts ~ 'Liebe'` | 1,142 | 12,721 | 12,721 | 12,722 | 26.64 | 26.52–29.64 |
| 6 | filter | `year = 1822 && annotation:fts ~ 'Goet'` | 8 | 611 | 10 | 11 | 1.92 | 1.85–2.14 |
| 7 | filter | `year >= 1800 && annotation:fts ~ 'Wien'` | 162 | 12,458 | 8,880 | 8,881 | 22.22 | 21.70–31.15 |
| 8 | filter | `year < 1800 && annotation:fts ~ 'Berlin'` | 146 | 4,732 | 1,465 | 1,466 | 9.78 | 9.02–10.25 |
| 9 | filter | `annotation:fts ~ 'Heinrich' \|\| year = 1822` | 2,394 | 0 | 133,948 | 133,949 | 83.03 | 82.38–105.68 |
| 10 | filter | `annotation:fts !~ 'Goet'` | 133,655 | 0 | 133,948 | 133,949 | 79.38 | 79.11–80.65 |
| 11 | filter | `preferred_title:fts ~ 'Goethe'` | 56 | 456 | 456 | 457 | 10.09 | 9.53–11.03 |
| 12 | filter | `preferred_title:fts ~ 'Schiller'` | 295 | 1,409 | 1,409 | 1,410 | 13.59 | 12.54–16.88 |
| 13 | filter | `preferred_title:fts ~ :ignorecase('goethe')` | 59 | 456 | 456 | 457 | 9.95 | 9.68–10.94 |
| 14 | filter | `preferred_title:fts ~ 'Gedicht'` | 1,810 | 66,631 | 66,631 | 66,632 | 74.36 | 68.51–76.78 |
| 15 | filter | `preferred_title:fts ~ 'A%buch'` | 12 | 7,653 | 7,653 | 7,654 | 22.12 | 21.04–23.41 |
| 16 | filter | `title_statement:fts ~ 'Verlag'` | 605 | 1,864 | 1,864 | 1,865 | 13.31 | 13.27–13.50 |
| 17 | filter | `title_statement:fts ~ :ignorecase('berlin')` | 759 | 4,732 | 4,732 | 4,733 | 22.84 | 18.37–24.46 |
| 18 | filter | `contributors.annotation:fts ?~ 'Goet'` | 0 | 611 | 611 | 611 | 11.34 | 10.48–11.40 |
| 19 | filter | `annotation:fts = 'Goethe'` | 0 | 0 | 133,948 | 133,948 | 80.17 | 77.42–80.22 |
| 20 | filter | `annotation:fts ~ 'XYZunlikely'` | 0 | 0 | 0 | 0 | 2.21 | 2.06–2.28 |
| 21 | word | `Goethe  ·  annotation` | 273 | 456 | 456 | 457 | 1.77 | 1.24–1.94 |
| 22 | word | `Schiller  ·  annotation` | 614 | 1,409 | 1,409 | 1,410 | 3.19 | 3.05–4.62 |
| 23 | word | `Goethe  ·  preferred_title` | 56 | 456 | 456 | 457 | 1.22 | 1.19–1.41 |
| 24 | word | `Liebe  ·  preferred_title` | 1,656 | 12,721 | 12,721 | 12,722 | 19.81 | 18.17–21.34 |
| 25 | word | `Goethe` | 433 | 456 | 456 | 457 | 1.66 | 1.61–1.75 |
| 26 | regex_clean | `Goet.*he  ·  annotation` | 284 | 579 | 579 | 580 | 1.61 | 1.57–1.87 |
| 27 | regex_clean | `Schill  ·  preferred_title` | 322 | 2,328 | 2,328 | 2,329 | 4.51 | 4.42–4.83 |
| 28 | regex_raw | `\*\*[^*]+\*\*  ·  annotation` | 3 | 1,143 | 1,143 | 1,144 | 2.42 | 2.10–2.46 |
| 29 | regex_raw | `<[^>]+>  ·  title_statement` | 0 | 133,948 | 133,948 | 133,948 | 161.90 | 159.62–171.50 |
| 30 | regex_clean | `Wien\|Berlin` | 3,490 | 16,344 | 16,344 | 16,345 | 29.12 | 28.32–29.39 |
