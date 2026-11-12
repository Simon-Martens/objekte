# Focused construction allocation profiles

The same ignored `bigram::tests::profile_single_worker_constructors` fixture builds 10,000 records in bulk mode under Valgrind DHAT. Control is clean scalar B `54dcac5c3b`; candidate is contiguous contribution `d7534b6d`. The debug test binary SHA-256 values are `b22e4e7cc893d16fc6c807e7313d84f82c89d765b427977d22f2bee50c2d4683` and `6c0a63836fe407f75e66cea9cafdb82945900b2f82af9279a6708513e436e42f`, respectively. Rust 1.99.1, Valgrind 3.27.1.

Build each role with `cargo test -p objekte-search-core --lib --locked --no-run`, then run the produced test binary with `OBJEKTE_BIGRAM_PROFILE_MODE=bulk OBJEKTE_BIGRAM_PROFILE_ALPHABET=ascii|unicode valgrind --tool=dhat --dhat-out-file=ROLE-ALPHABET.dhat --log-file=ROLE-ALPHABET.log BINARY --ignored --exact bigram::tests::profile_single_worker_constructors --nocapture > ROLE-ALPHABET.stdout`. The raw Valgrind logs and test stdout are archived as `.gz` files without rewriting their contents.

| Alphabet | Role | Gross allocated bytes | Allocation blocks | Peak live bytes |
| --- | --- | ---: | ---: | ---: |
| ASCII | B | 3,949,868 | 71,545 | 2,507,547 |
| ASCII | Contiguous | 4,364,008 | 71,116 | 2,670,823 |
| Unicode | B | 4,375,046 | 71,963 | 2,691,291 |
| Unicode | Contiguous | 4,665,634 | 71,380 | 2,796,703 |

The candidate removes 429 ASCII and 583 Unicode allocation blocks in this fixture, but gross bytes and peak live bytes rise. These are process-wide debug-test numbers including the harness and fixture; they do not represent full-corpus peak RSS. Both roles produced the same final key counts (66 ASCII, 88 Unicode) and estimated persistent bigram bytes (433,760 ASCII, 611,360 Unicode). The candidate's stdout records its actual construction type sizes and retained-capacity lower bound. Its separate full-corpus profiles show lower construction capacity and RSS despite this small-fixture peak-live increase.
