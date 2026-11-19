use super::*;

#[test]
fn borrowed_decoder_uses_source_slice_or_reused_storage() {
    let plain = "\"Straße\"".as_bytes();
    let escaped = br#""before\u2139after\ud83d\ude80""#;
    let mut decoded = String::new();
    with_quoted_text(plain, 0..plain.len(), &mut decoded, |text| {
        assert_eq!(text.as_ptr(), plain[1..].as_ptr());
        assert_eq!(text, "Straße");
        Ok(())
    })
    .unwrap();
    assert_eq!(decoded.capacity(), 0);
    with_quoted_text(escaped, 0..escaped.len(), &mut decoded, |text| {
        assert_eq!(text, "beforeℹafter🚀");
        Ok(())
    })
    .unwrap();
    let capacity = decoded.capacity();
    let pointer = decoded.as_ptr();
    for token in [br#""\u0061""#.as_slice(), br#""\n""#, br#""\u00df""#] {
        with_quoted_text(token, 0..token.len(), &mut decoded, |_| Ok(())).unwrap();
        assert_eq!(decoded.as_ptr(), pointer);
        assert_eq!(decoded.capacity(), capacity);
    }
}

#[test]
#[ignore = "release-only decoder comparison; run serially with --nocapture"]
fn decoder_strategy_microbench() {
    use std::hint::black_box;
    use std::time::Instant;

    let cases = [
        ("plain-ascii", format!("\"{}\"", "a".repeat(8192))),
        ("plain-unicode", format!("\"{}\"", "Straßeℹ".repeat(1024))),
        (
            "sparse-simple",
            format!("\"{}\"", format!("{}\\n", "a".repeat(1024)).repeat(8)),
        ),
        ("dense-simple", format!("\"{}\"", "a\\n".repeat(2048))),
        ("unicode-escape", format!("\"{}\"", "\\u2139".repeat(1024))),
        (
            "surrogate-pair",
            format!("\"{}\"", "\\ud83d\\ude80".repeat(512)),
        ),
        ("alternating", format!("\"{}\"", "a\\n".repeat(2048))),
    ];
    let short = br#""x\n""#;
    for (name, source) in cases {
        let token = source.as_bytes();
        let expected: String = serde_json::from_slice(token).unwrap();
        for strategy in [0, 1, 2, 3] {
            let mut decoded = String::new();
            let (_, actual) = match strategy {
                0 => with_next_quoted_text_strategy::<0, _>(token, 0, &mut decoded, |text| {
                    Ok(text.to_owned())
                }),
                1 => with_next_quoted_text_strategy::<1, _>(token, 0, &mut decoded, |text| {
                    Ok(text.to_owned())
                }),
                2 => with_next_quoted_text_strategy::<2, _>(token, 0, &mut decoded, |text| {
                    Ok(text.to_owned())
                }),
                _ => with_next_quoted_text_strategy::<3, _>(token, 0, &mut decoded, |text| {
                    Ok(text.to_owned())
                }),
            }
            .unwrap();
            assert_eq!(actual, expected);
        }
        let iterations = (8 * 1024 * 1024 / token.len()).max(128);
        for (label, strategy) in [
            ("repeated-memchr", 0),
            ("first-memchr-byte-loop", 1),
            ("adaptive-8", 2),
            ("adaptive-4", 3),
            ("predecessor-serde-owned", 4),
        ] {
            let mut samples = Vec::new();
            for round in 0..6 {
                let mut decoded = String::with_capacity(expected.len());
                let started = Instant::now();
                for iteration in 0..iterations {
                    let input = if name == "alternating" && iteration % 2 == 0 {
                        short.as_slice()
                    } else {
                        token
                    };
                    match strategy {
                        0 => {
                            let (_, length) = with_next_quoted_text_strategy::<0, _>(
                                black_box(input),
                                0,
                                &mut decoded,
                                |text| Ok(text.len()),
                            )
                            .unwrap();
                            black_box(length);
                        }
                        1 => {
                            let (_, length) = with_next_quoted_text_strategy::<1, _>(
                                black_box(input),
                                0,
                                &mut decoded,
                                |text| Ok(text.len()),
                            )
                            .unwrap();
                            black_box(length);
                        }
                        2 => {
                            let (_, length) = with_next_quoted_text_strategy::<2, _>(
                                black_box(input),
                                0,
                                &mut decoded,
                                |text| Ok(text.len()),
                            )
                            .unwrap();
                            black_box(length);
                        }
                        3 => {
                            let (_, length) = with_next_quoted_text_strategy::<3, _>(
                                black_box(input),
                                0,
                                &mut decoded,
                                |text| Ok(text.len()),
                            )
                            .unwrap();
                            black_box(length);
                        }
                        _ => {
                            let value = serde_json::Deserializer::from_slice(black_box(input))
                                .into_iter::<String>()
                                .next()
                                .unwrap()
                                .unwrap();
                            black_box(value);
                        }
                    }
                }
                if round > 0 {
                    samples.push(started.elapsed().as_nanos() / iterations as u128);
                }
            }
            samples.sort_unstable();
            println!(
                "decoder {name} {label}: {} ns/token median of 5",
                samples[2]
            );
        }
    }
}
