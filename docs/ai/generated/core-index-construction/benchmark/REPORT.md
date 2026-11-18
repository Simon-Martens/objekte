# Core index construction benchmark

- Task: `core-index-construction`
- Base: `407410cbc28ae2a2c6fa89ba0e6af2458157c472`
- Corpus: `/home/simon/source/objekte/examples/musenalm`
- Alphabet: ASCII
- Build workers: 32
- Host available parallelism: 32
- Workload: one warm-up per binary, then three measured builds per revision in alternating order

| Metric | Base median | Current median | Delta |
| --- | ---: | ---: | ---: |
| Wall time | 641,217,095 ns | 627,727,423 ns | -2.11% |
| Build time | 641,197,306 ns | 613,688,920 ns | -4.29% |
| Discovery time | 188,338,306 ns | 196,704,760 ns | +4.44% |
| Peak resident bytes | 655,540,224 | 646,385,664 | -1.40% |
| Peak admitted bytes | 523,665,408 | 523,665,408 | 0% |
| Inventory heap bytes | 40,275,632 | 40,275,632 | 0% |
| Bigram heap bytes | 86,292,620 | 86,292,620 | 0% |
| Index-family bytes | 110,734,672 | 110,734,672 | 0% |
| Peak concurrent segment jobs | 15 | 15 | 0 |

All measured runs reported:

- Index signature: `4a38808629a1544a354d715e42c85ac426b1cfe93376e862253b3d5cc03f349f`
- Schema manifest digest: `38f2dc5112146aa6ac0514291c76eaeb741421fa8a4624e50fa83c4c0a449bf9`
- Records: 148,486
- Build parses: 148,486
- Failed parses: 0
- Resolved build workers: 32

The timing differences are within the observed run spread and are not treated as a repeatable regression. Segment counts, registrations, source-byte admission, index signatures and parse counts match exactly. Raw JSON measurements are stored next to this report.
