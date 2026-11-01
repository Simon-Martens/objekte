//! Candidate-planner correctness tests.
use super::*;
use crate::bigram::BigramIndexBuilder;

/// Build a tiny index from the given file contents for testing.
fn build_test_index(files: &[&[u8]]) -> BigramFilter {
    let n = files.len();
    let consec_builder = BigramIndexBuilder::new(n);
    let skip_builder = BigramIndexBuilder::new(n);
    for (i, content) in files.iter().enumerate() {
        consec_builder.add_file_content(&skip_builder, i, content);
    }
    let mut idx = consec_builder.compress(Some(0));
    idx.set_skip_index(skip_builder.compress(Some(0)));
    idx
}

#[test]
fn regex_candidates_cover_every_matching_short_byte_string() {
    let alphabet = b"abAB1.\\";
    let mut values = vec![Vec::new()];
    for length in 1..=4 {
        let count = alphabet.len().pow(length);
        for mut number in 0..count {
            let mut value = vec![0; length as usize];
            for byte in &mut value {
                *byte = alphabet[number % alphabet.len()];
                number /= alphabet.len();
            }
            values.push(value);
        }
    }
    let refs = values.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let index = build_test_index(&refs);
    for pattern in [
        "ab",
        "a.b",
        "a.*b",
        "(ab)?",
        "a[bB]1",
        "(?i:ab)",
        "ab|B1",
        "a.{0,2}b",
        "a[ .]b",
        "a\\d1",
        "^ab$",
        "[aB]b",
        "a..b",
        "a[bB]?1",
        "a\\.b",
        "(?i:a.*1)",
    ] {
        let regex = regex::bytes::RegexBuilder::new(pattern)
            .unicode(false)
            .build()
            .unwrap();
        let bits = super::candidates(&index, pattern);
        for (slot, value) in values.iter().enumerate() {
            if regex.is_match(value) {
                assert!(
                    bits.as_ref()
                        .is_none_or(|bits| BigramFilter::is_candidate(bits, slot)),
                    "pattern {pattern:?} missed {value:?}"
                );
            }
        }
    }
}

#[test]
fn literal_pattern() {
    let idx = build_test_index(&[
        b"hello world",     // 0: contains "hello"
        b"goodbye world",   // 1: no "hello"
        b"say hello there", // 2: contains "hello"
    ]);

    let q = regex_to_bigram_query("hello");
    assert!(!q.is_any());

    let candidates = q.evaluate(&idx).unwrap();
    assert!(BigramFilter::is_candidate(&candidates, 0));
    assert!(!BigramFilter::is_candidate(&candidates, 1));
    assert!(BigramFilter::is_candidate(&candidates, 2));
}

#[test]
fn alternation() {
    let idx = build_test_index(&[
        b"has foo in it", // 0
        b"has bar in it", // 1
        b"has xyz in it", // 2
    ]);

    let q = regex_to_bigram_query("foo|bar");
    assert!(!q.is_any());

    let candidates = q.evaluate(&idx).unwrap();
    assert!(BigramFilter::is_candidate(&candidates, 0));
    assert!(BigramFilter::is_candidate(&candidates, 1));
    // xyz doesn't contain foo or bar bigrams
    assert!(!BigramFilter::is_candidate(&candidates, 2));
}

#[test]
fn wildcard_concat() {
    let idx = build_test_index(&[
        b"foo something bar", // 0
        b"foo only",          // 1: has foo but not bar
        b"only bar",          // 2: has bar but not foo
    ]);

    let q = regex_to_bigram_query("foo.*bar");
    assert!(!q.is_any());

    let candidates = q.evaluate(&idx).unwrap();
    assert!(BigramFilter::is_candidate(&candidates, 0));
    // file 1 and 2 should be filtered (missing bigrams from "bar" / "foo")
    assert!(!BigramFilter::is_candidate(&candidates, 1));
    assert!(!BigramFilter::is_candidate(&candidates, 2));
}

#[test]
fn sparse1_across_dot() {
    // "a.b" should produce a skip-1 bigram (a,b)
    let idx = build_test_index(&[
        b"axb", // 0: has sparse-1 (a,b)
        b"ayb", // 1: has sparse-1 (a,b)
        b"xyz", // 2: no (a,b) at all
    ]);

    let q = regex_to_bigram_query("a.b");
    assert!(!q.is_any());

    let candidates = q.evaluate(&idx).unwrap();
    assert!(BigramFilter::is_candidate(&candidates, 0));
    assert!(BigramFilter::is_candidate(&candidates, 1));
    assert!(!BigramFilter::is_candidate(&candidates, 2));
}

#[test]
fn sparse1_across_digit() {
    // "foo\dbar" -> sparse-1 (o,b) across \d
    let idx = build_test_index(&[
        b"foo3bar baz", // 0: has all bigrams
        b"foobar baz",  // 1: has consecutive (o,b) but pattern needs sparse-1
        b"xyz only",    // 2: no relevant bigrams
    ]);

    let q = regex_to_bigram_query(r"foo\dbar");
    assert!(!q.is_any());

    let candidates = q.evaluate(&idx).unwrap();
    assert!(BigramFilter::is_candidate(&candidates, 0));
    // file 1 may or may not match depending on what bigrams are in the index
    // (it has all the literal bigrams and also o,b as both consec and skip-1)
    // The important thing is file 2 is excluded:
    assert!(!BigramFilter::is_candidate(&candidates, 2));
}

#[test]
fn pure_wildcard_is_any() {
    let q = regex_to_bigram_query(".*");
    assert!(q.is_any());
}

#[test]
fn single_char_is_any() {
    let q = regex_to_bigram_query("a");
    assert!(q.is_any());
}

#[test]
fn invalid_regex_is_any() {
    let q = regex_to_bigram_query("[invalid");
    assert!(q.is_any());
}

#[test]
fn optional_group_excluded() {
    let q = regex_to_bigram_query("foo(bar)?baz");
    assert!(!q.is_any());

    let idx = build_test_index(&[
        b"foobaz content",    // 0: has foo+baz bigrams (bar absent)
        b"foobarbaz content", // 1: has everything
        b"xyz only",          // 2: nothing
    ]);

    let candidates = q.evaluate(&idx).unwrap();
    assert!(BigramFilter::is_candidate(&candidates, 0));
    assert!(BigramFilter::is_candidate(&candidates, 1));
    assert!(!BigramFilter::is_candidate(&candidates, 2));
}

#[test]
fn repetition_min2_cross_boundary() {
    // (ab){2,} -> bigram "ab" + cross-boundary "b","a"
    let q = regex_to_bigram_query("(ab){2,}");
    assert!(!q.is_any());

    let idx = build_test_index(&[
        b"ababab", // 0: has "ab" and "b"->"a"
        b"abonly", // 1: has "ab" but not "b"->"a"
        b"xyz",    // 2: nothing
    ]);

    let candidates = q.evaluate(&idx).unwrap();
    assert!(BigramFilter::is_candidate(&candidates, 0));
    assert!(!BigramFilter::is_candidate(&candidates, 2));
}

#[test]
fn two_dots_no_sparse1() {
    let q = regex_to_bigram_query("a..b");
    // Single-char literals with 2 unknown bytes between -> Any
    assert!(q.is_any());
}

#[test]
fn character_class_cross_boundary() {
    // [abc]de -> cross-boundary OR(ad,bd,cd) + bigram de
    // All three class variants must appear in the corpus so the OR
    // branches are tracked in the index (untracked bigrams make the
    // OR conservatively return None, which is correct but untestable).
    let idx = build_test_index(&[
        b"ade content", // 0: has ad
        b"bde content", // 1: has bd
        b"cde content", // 2: has cd
        b"xde content", // 3: has de but not ad/bd/cd
    ]);

    let q = regex_to_bigram_query("[abc]de");
    assert!(!q.is_any());

    let candidates = q.evaluate(&idx).unwrap();
    assert!(BigramFilter::is_candidate(&candidates, 0));
    assert!(BigramFilter::is_candidate(&candidates, 1));
    assert!(BigramFilter::is_candidate(&candidates, 2));
    // file 3 doesn't have ad/bd/cd so should be filtered
    assert!(!BigramFilter::is_candidate(&candidates, 3));
}

fn has_consec(q: &BigramQuery, a: u8, b: u8) -> bool {
    let Some(key) = consec_key(a, b) else {
        return false;
    };
    match q {
        BigramQuery::Consec(k) => *k == key,
        BigramQuery::And(cs) | BigramQuery::Or(cs) => cs.iter().any(|c| has_consec(c, a, b)),
        _ => false,
    }
}

fn has_skip1(q: &BigramQuery, a: u8, b: u8) -> bool {
    let Some(key) = consec_key(a, b) else {
        return false;
    };
    match q {
        BigramQuery::Skip1(k) => *k == key,
        BigramQuery::And(cs) | BigramQuery::Or(cs) => cs.iter().any(|c| has_skip1(c, a, b)),
        _ => false,
    }
}

/// Bigram expectation: `("ab", is_skip1)`.
/// The 2-char str is the byte pair; C = consecutive, S = skip-1.
type Bg = (&'static str, bool);
const C: bool = false;
const S: bool = true;

/// Top 15+ commonly used regex patterns from
/// https://digitalfortress.tech/tips/top-15-commonly-used-regex/
/// plus typical grep patterns used by agentic tools.
///
/// Each entry: `(regex, Option<&[Bg]>)`.
///   - `None`        -> pure classes / unsupported syntax, Any is acceptable.
///   - `Some(&[..])` -> must be non-Any, and every listed bigram must appear.
#[test]
fn common_regex_patterns() {
    #[rustfmt::skip]
    let cases: &[(&str, Option<&[Bg]>)] = &[
        (r"^\d+$",                                                      None), // 1.  whole numbers
        (r"^\d*\.\d+$",                                                 None), // 2.  decimals
        (r"^\d*(\.\d+)?$",                                              None), // 3.  whole + decimal
        (r"^-?\d*(\.\d+)?$",                                            None), // 4.  neg/pos decimal
        (r"[-]?[0-9]+[,.]?[0-9]*([/][0-9]+[,.]?[0-9]*)*",             None), // 5.  fractions
        (r"^[a-zA-Z0-9]*$",                                             None), // 6.  alphanumeric
        (r"^[a-zA-Z0-9 ]*$",                                            None), // 7.  alphanum + space
        (r"^([a-zA-Z0-9._%-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,6})*$",      None), // 8.  email
        (r"^([a-z0-9_\.\+-]+)@([\da-z\.-]+)\.([a-z\.]{2,6})$",         None), // 9.  email v2
        (r"(?=(.*[0-9]))(?=.*[!@#$%^&*()\[\]{}\-_+=~`|:;<>,./?\x5c])(?=.*[a-z])(?=(.*[A-Z]))(?=(.*)).{8,}", None), // 10. complex pw
        (r"(?=(.*[0-9]))((?=.*[A-Za-z0-9])(?=.*[A-Z])(?=.*[a-z]))^.{8,}$", None), // 11. moderate pw
        (r"^[a-z0-9_-]{3,16}$",                                        None), // 12. username
        (r"(https?://)?(www\.)?[-a-zA-Z0-9@:%._\+~#=]{2,256}\.[a-z]{2,6}\b([-a-zA-Z0-9@:%_\+.~#?&//=]*)", None), // 14. URL optional
        (r"^(([0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])\.){3}([0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])$", None), // 15. IPv4
        (r"(([0-9a-fA-F]{1,4}:){7,7}[0-9a-fA-F]{1,4}|([0-9a-fA-F]{1,4}:){1,7}:|([0-9a-fA-F]{1,4}:){1,6}:[0-9a-fA-F]{1,4}|([0-9a-fA-F]{1,4}:){1,5}(:[0-9a-fA-F]{1,4}){1,2}|([0-9a-fA-F]{1,4}:){1,4}(:[0-9a-fA-F]{1,4}){1,3}|([0-9a-fA-F]{1,4}:){1,3}(:[0-9a-fA-F]{1,4}){1,4}|([0-9a-fA-F]{1,4}:){1,2}(:[0-9a-fA-F]{1,4}){1,5}|[0-9a-fA-F]{1,4}:((:[0-9a-fA-F]{1,4}){1,6})|:((:[0-9a-fA-F]{1,4}){1,7}|:)|fe80:(:[0-9a-fA-F]{0,4}){0,4}%[0-9a-zA-Z]{1,}|::(ffff(:0{1,4}){0,1}:){0,1}((25[0-5]|(2[0-4]|1{0,1}[0-9]){0,1}[0-9])\.){3,3}(25[0-5]|(2[0-4]|1{0,1}[0-9]){0,1}[0-9])|([0-9a-fA-F]{1,4}:){1,4}:((25[0-5]|(2[0-4]|1{0,1}[0-9]){0,1}[0-9])\.){3,3}(25[0-5]|(2[0-4]|1{0,1}[0-9]){0,1}[0-9]))", None), // 16. IPv6
        (r"[12]\d{3}-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])",          None), // 17. date
        (r"^(0?[1-9]|1[0-2]):[0-5][0-9]$",                            None), // 18. time 12h
        (r"((1[0-2]|0?[1-9]):([0-5][0-9]) ?([AaPp][Mm]))",            None), // 19. time AM/PM
        (r"^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$",                     None), // 20. time 24h
        (r"^([0-9]|0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$",               None), // 21. time 24h v2
        (r"(?:[01]\d|2[0123]):(?:[012345]\d):(?:[012345]\d)",          None), // 22. time+sec
        (r"</?[\w\s]*>|<.+[\W]>",                                      None), // 23. HTML tag
        (r"\bon\w+=\S+(?=.*>)",                                        None), // 24. inline JS
        (r"^[a-z0-9]+(?:-[a-z0-9]+)*$",                               None), // 25. slug
        (r"(\b\w+\b)(?=.*\b\1\b)",                                    None), // 26. dup words
        (r"^[\w,\s-]+\.[A-Za-z]{3}$",                                 None), // 27. filename
        (r"^[A-PR-WY][1-9]\d\s?\d{4}[1-9]$",                         None), // 28. HK ID

        // 13. URL with required protocol
        (r"https?://(www\.)?[-a-zA-Z0-9@:%._\+~#=]{2,256}\.[a-z]{2,6}\b([-a-zA-Z0-9@:%_\+.~#?&//=]*)", Some(&[
            ("ht", C), ("tt", C), ("tp", C),    // from "http"
            ("ht", S), ("tp", S),               // from "http" skip-1
            (":/", C), ("//", C),               // from "://"
        ])),

        // 29. fn\s+\w+
        (r"fn\s+\w+", Some(&[
            ("fn", C),                          // from "fn"
            ("n ", C),                          // cross-boundary: 'n' -> \s starts ' '
        ])),

        // 30. use\s+crate::
        (r"use\s+crate::", Some(&[
            ("us", C), ("se", C), ("ue", S),    // from "use"
            ("cr", C), ("ra", C), ("at", C),    // from "crate"
            ("te", C), ("::", C),
            ("ca", S), ("rt", S), ("ae", S),    // "crate" skip-1
        ])),

        // 31. unwrap\(\)|expect\(
        (r"unwrap\(\)|expect\(", Some(&[
            ("nw", C), ("wr", C), ("ra", C),    // "unwrap("
            ("ap", C), ("p(", C),
            ("xp", C), ("pe", C), ("ec", C),    // "expect("
            ("ct", C), ("t(", C),
        ])),

        // 32. TODO|FIXME|HACK
        (r"TODO|FIXME|HACK", Some(&[
            ("to", C), ("od", C), ("do", C),    // "TODO"
            ("fi", C), ("ix", C), ("xm", C),    // "FIXME"
            ("me", C),
            ("ha", C), ("ac", C), ("ck", C),    // "HACK"
            ("hc", S), ("ak", S),               // "HACK" skip-1
        ])),
    ];

    for (i, &(pattern, expected)) in cases.iter().enumerate() {
        let q = regex_to_bigram_query(pattern);

        if let Some(bigrams) = expected {
            assert!(
                !q.is_any(),
                "#{i} {pattern:?}: expected bigrams but got Any"
            );

            for &(pair, skip) in bigrams {
                let b = pair.as_bytes();
                debug_assert_eq!(b.len(), 2, "bigram must be 2 chars: {pair:?}");
                let found = if skip {
                    has_skip1(&q, b[0], b[1])
                } else {
                    has_consec(&q, b[0], b[1])
                };
                let kind = if skip { "skip-1" } else { "consec" };
                assert!(found, "#{i} {pattern:?}: missing {kind} bigram {pair:?}");
            }
        }
    }
}
