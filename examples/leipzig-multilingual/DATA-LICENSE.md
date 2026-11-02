# Data attribution and license

The sentences in `data/texts/` are adapted from the Leipzig Corpora Collection
by the Natural Language Processing Group at Leipzig University. The source is
the eight 2020 news archives listed with download links and SHA-256 hashes in
the [benchmark report](../../crates/bench/BIGRAM-FOREIGN-50-2026-11-02.md#source-archives).

The [Leipzig terms of usage](https://wortschatz.uni-leipzig.de/en/usage)
state that downloadable corpora are licensed under Creative Commons Attribution
(CC BY). The terms distinguish downloaded corpora from the website's query
service. The upstream terms do not specify a CC BY version here. This data
remains subject to its upstream license; the repository's MIT code license does
not replace it.

We selected the first 50,000 sentences from each 100,000-sentence archive and
converted each sentence into a single JSON `text` value. The source sentence
number and language code became the filename. The sentence text was preserved;
JSON escaping and UTF-8 encoding are the only representation changes.

Please cite the source as requested by the Leipzig Corpora Collection:

> D. Goldhahn, T. Eckart, and U. Quasthoff. “Building Large Monolingual
> Dictionaries at the Leipzig Corpora Collection: From 100 to 200 Languages.”
> Proceedings of LREC 2012.
