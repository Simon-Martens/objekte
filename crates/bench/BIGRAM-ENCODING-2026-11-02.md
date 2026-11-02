# ASCII and UTF-8 byte bigram indexes — 2 November 2026

`bigram-encoding-compare` built a fresh `FilePicker` in each process. Each cell
below is the median of three independent release runs. Candidate query times
are medians of 21 calls per process, then the median across processes. The
queries measure posting selection and candidate ID collection, **not** source
verification or final search latency. The Linux OS file cache was warm and
uncontrolled. Hardware: Intel Core i9-13900K, x86-64.

The Musenalm `contents` collection has 133,948 JSON files. The synthetic corpus
has 24,000 JSON files cycling through German, French, Spanish, Polish,
Cyrillic, Chinese, Japanese, Arabic, and emoji text. `VmHWM` is peak **process**
resident memory through construction, including the builder and other picker
state. Compressed bytes are the finished bigram filter and skip index in memory;
snapshot bytes include the file manifest, checksum, and encoded filter.

| Dataset | Encoding | Build ms | Peak RSS MiB | Compressed MiB | Snapshot MiB | Save ms | Restore ms |
|---|---|---:|---:|---:|---:|---:|---:|
| Musenalm | ASCII | 356.9 | 196.0 | 17.1 | 26.1 | 275.8 | 68.4 |
| Musenalm | UTF-8 bytes | 478.1 | 248.6 | 18.1 | 27.2 | 278.7 | 70.7 |
| Multilingual 24k | ASCII | 40.4 | 24.1 | 0.214 | 1.61 | 37.4 | 4.79 |
| Multilingual 24k | UTF-8 bytes | 55.2 | 26.0 | 0.594 | 1.99 | 38.5 | 5.54 |

On Musenalm, UTF-8 construction took 34% longer, used 27% more peak resident
memory, and produced a 6% larger compressed filter. On the synthetic corpus,
the compressed filter was 2.8 times larger because its repeated non-ASCII text
contributes additional postings. Snapshot save and restore were close on
Musenalm; the small synthetic corpus showed a 0.75 ms restore increase.

## Candidate queries

Each entry is `candidate files / milliseconds`. Smaller candidate sets reduce
the later source verification work. They do not necessarily make this
candidate-only benchmark faster because collecting IDs has its own cost.

| Query | Musenalm ASCII | Musenalm UTF-8 | Multilingual ASCII | Multilingual UTF-8 |
|---|---:|---:|---:|---:|
| `Wasser` | 3,660 / 0.204 | 3,660 / 0.208 | 0 / 0.023 | 0 / 0.025 |
| `Berlin` | 4,899 / 0.253 | 4,899 / 0.251 | 2,000 / 0.031 | 2,000 / 0.031 |
| `Müller` | 13,148 / 0.529 | 1,108 / 0.147 | 2,000 / 0.036 | 2,000 / 0.045 |
| `Straße` | 18,310 / 0.686 | 561 / 0.136 | 2,000 / 0.033 | 2,000 / 0.042 |
| `français` | 1,078 / 0.126 | 27 / 0.122 | 2,000 / 0.036 | 2,000 / 0.045 |
| `中文` | 133,948 / 1.315 | 133,948 / 1.314 | 24,000 / 0.196 | 2,000 / 0.041 |
| `日本語` | 133,948 / 1.324 | 133,948 / 1.322 | 24,000 / 0.189 | 2,000 / 0.050 |

Musenalm has no matching Chinese or Japanese text in this collection, and its
compressed index omits their unobserved pairs. A query without a tracked
posting conservatively admits every file. The synthetic corpus demonstrates
that observed CJK byte pairs narrow those terms from 24,000 to 2,000 candidates.

For an ASCII regression check, the same benchmark was run three times against
the parent commit `7f90be6b85` with the old ASCII implementation. Its Musenalm
build median was 373.7 ms, compressed size was the same 17,913,583 bytes, and
`Wasser` candidates took 0.325 ms. The new ASCII path took 356.9 ms and
0.204 ms respectively. A no-overlay candidate fast path was added after an
initial measurement showed a query regression; the final numbers above include
that change. All measurements are short warm-cache runs, so they show the
relative behavior on this machine rather than a cross-device guarantee.

Reproduce with the commands in [README.md](README.md). Tests compare exact
indexed and source-scan result sets separately; these benchmark figures only
measure index construction, storage, restoration, and candidate selection.
