# Construction with foreground queries and a held view

Nine fresh-process warm Musenalm runs alternated `idle`, `foreground`, and
`held-view` modes three times on integration revision
`c7959d4f557b0190772b7bd635655a8612925057`. Each used the same absolute
corpus root, configured schemas, ASCII bigrams and 32 build workers. The
foreground mode repeatedly ran Q108, Q116 and Q120 against available views,
then paused 2 ms between three-query cycles. The held-view mode pinned the
first published view until construction completed. `campaign.json` and the
nine individual JSON files retain all measurements.

| Mode | Median build | Build range | Median high-water RSS | Median post-build RSS |
| --- | ---: | ---: | ---: | ---: |
| Idle | 908.074 ms | 899.075–925.119 ms | 564,637,696 B | 553,947,136 B |
| Foreground | 941.294 ms | 925.418–945.734 ms | 563,625,984 B | 553,132,032 B |
| Held view | 916.280 ms | 896.460–917.711 ms | 564,891,648 B | 555,036,672 B |

Foreground queries increased median construction time by 3.66% relative to
idle. Each foreground process completed 834–900 queries; per-process median
query latency was 44–52 µs and p95 was 91–117 µs. Query source opens were
zero. Each run still made exactly 148,486 construction reads and parses of
193,297,230 bytes. The held view remained at revision 2 while later
publications completed; its median high-water RSS was only 253,952 B above
the idle median. RSS immediately after releasing the held view changed by
at most 49,152 B in these runs. Those RSS observations are allocator-dependent;
the high-water comparison provides the stronger evidence against retained
full-view copies.

Command for each run:

```text
target/release/examples/bench_initial_concurrency --root /home/simon/source/objekte/examples/musenalm --mode MODE --output bench/results/incremental-index-availability/2026-11-13-concurrency/NN-MODE.json
```

This campaign predates a possible complete-query latency repair. Rerun it if
that repair changes the construction or published-view path.
