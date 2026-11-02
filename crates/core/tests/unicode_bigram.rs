use objekte_core::bigram::{
    BigramEncoding, BigramFilter, BigramIndexBuilder, BigramOverlay,
    build_bigram_index_from_content_with_encoding, extract_bigrams_with_encoding,
};
use objekte_core::{
    CacheMode, JsonQuery, JsonQueryBackend, Options, Repository, TextMode, TextQuery,
};
use std::{collections::BTreeSet, fs};

fn ids<I: IntoIterator<Item = String>>(items: I) -> BTreeSet<String> {
    items.into_iter().collect()
}

fn open(root: &std::path::Path, encoding: BigramEncoding, cache: CacheMode) -> Repository {
    let mut options = Options::new();
    options.bigram_encoding = encoding;
    options.cache = cache;
    Repository::builder(root).options(options).open().unwrap()
}

#[test]
fn utf8_pairs_cover_scripts_and_skip_one_bytes() {
    let samples = [
        "Müller",
        "Straße",
        "Äpfel",
        "français",
        "œuvre",
        "niño",
        "Łódź",
        "Привет",
        "中文",
        "日本語",
        "مرحبا",
        "😀🐈",
    ];
    for sample in samples {
        let index = build_bigram_index_from_content_with_encoding(
            &[Some(sample.as_bytes()), Some(b"unrelated")],
            BigramEncoding::Utf8Bytes,
        );
        assert_eq!(index.encoding(), BigramEncoding::Utf8Bytes);
        let expected = sample.as_bytes().windows(2).map(|pair| {
            u16::from_be_bytes([pair[0].to_ascii_lowercase(), pair[1].to_ascii_lowercase()])
        });
        let extracted = extract_bigrams_with_encoding(sample.as_bytes(), BigramEncoding::Utf8Bytes);
        for key in expected {
            assert!(extracted.contains(&key), "{sample}: missing {key:04x}");
        }
        let hits = index.query(sample.as_bytes()).unwrap();
        assert!(BigramFilter::is_candidate(&hits, 0), "{sample}");
        assert!(!BigramFilter::is_candidate(&hits, 1), "{sample}");
        let skip = index.skip_index().unwrap();
        for pair in sample.as_bytes().windows(3) {
            let key =
                u16::from_be_bytes([pair[0].to_ascii_lowercase(), pair[2].to_ascii_lowercase()]);
            // Non-ASCII skip pairs here occur only in this file and must be kept.
            if pair[0] >= 128 || pair[2] >= 128 {
                assert!(skip.has_key(key), "{sample}: missing skip pair {key:04x}");
            }
            if skip.has_key(key) {
                assert!(skip.query(&[pair[0], pair[2]]).is_some());
            }
        }
    }
}

#[test]
fn utf8_filter_and_overlay_roundtrip_without_ascii_regression() {
    let content = [
        Some("Müller".as_bytes()),
        Some("中文".as_bytes()),
        Some(b"ASCII".as_slice()),
    ];
    for encoding in [BigramEncoding::Ascii, BigramEncoding::Utf8Bytes] {
        let filter = build_bigram_index_from_content_with_encoding(&content, encoding);
        let saved = filter.to_bytes().unwrap();
        assert_eq!(&saved[..8], b"OBJBG002");
        let restored = BigramFilter::from_bytes(&saved).unwrap();
        assert_eq!(restored.encoding(), encoding);
        for term in ["Müller", "中文", "ASCII"] {
            assert_eq!(
                filter.query(term.as_bytes()),
                restored.query(term.as_bytes())
            );
        }
        let mut overlay = BigramOverlay::new_with_encoding(content.len(), encoding);
        overlay.modify_file(1, "Łódź".as_bytes());
        let keys = extract_bigrams_with_encoding("Łódź".as_bytes(), encoding);
        assert_eq!(overlay.query_modified(&keys), vec![1]);
        overlay.delete_file(1);
        assert!(overlay.query_modified(&keys).is_empty());
    }
}

#[test]
fn unicode_regex_planner_narrows_observed_non_ascii_literals() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("a.json"), r#"{"title":"中文"}"#).unwrap();
    fs::write(temp.path().join("b.json"), r#"{"title":"日本語"}"#).unwrap();
    fs::write(temp.path().join("c.json"), r#"{"title":"مرحبا"}"#).unwrap();
    let ascii = objekte_core::file_picker::FilePicker::open_with_encoding(
        temp.path(),
        BigramEncoding::Ascii,
    )
    .unwrap();
    let utf8 = objekte_core::file_picker::FilePicker::open_with_encoding(
        temp.path(),
        BigramEncoding::Utf8Bytes,
    )
    .unwrap();
    assert_eq!(ascii.regex_candidate_ids("中文").len(), 3);
    assert_eq!(utf8.regex_candidate_ids("中文"), vec!["a"]);
}

#[test]
fn every_multilingual_substring_keeps_its_source_candidate() {
    let samples = [
        "Müller Straße Äpfel",
        "français œuvre niño",
        "Łódź Привет 中文",
        "日本語 مرحبا 😀🐈",
        "ASCII\u{1f}boundary",
    ];
    let contents = samples
        .iter()
        .map(|sample| Some(sample.as_bytes()))
        .collect::<Vec<_>>();
    for encoding in [BigramEncoding::Ascii, BigramEncoding::Utf8Bytes] {
        let built = build_bigram_index_from_content_with_encoding(&contents, encoding);
        let restored = BigramFilter::from_bytes(&built.to_bytes().unwrap()).unwrap();
        for (slot, sample) in samples.iter().enumerate() {
            let mut boundaries = sample.char_indices().map(|(at, _)| at).collect::<Vec<_>>();
            boundaries.push(sample.len());
            for start in 0..boundaries.len() - 1 {
                for end in start + 1..(start + 5).min(boundaries.len()) {
                    let substring = &sample[boundaries[start]..boundaries[end]];
                    for filter in [&built, &restored] {
                        if let Some(bits) = filter.query(substring.as_bytes()) {
                            assert!(
                                BigramFilter::is_candidate(&bits, slot),
                                "{encoding:?} lost {substring:?} from {sample:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn unicode_repository_matches_direct_scans_and_rebuilds_on_mode_change() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data/docs");
    fs::create_dir_all(&data).unwrap();
    for (id, title) in [
        ("a", "Müller"),
        ("b", "Straße"),
        ("c", "Äpfel"),
        ("d", "français"),
        ("e", "œuvre"),
        ("f", "niño"),
        ("g", "Łódź"),
        ("h", "Привет"),
        ("i", "中文"),
        ("j", "日本語"),
        ("k", "مرحبا"),
        ("l", "😀🐈"),
        ("m", "line\u{1f}break"),
    ] {
        fs::write(
            data.join(format!("{id}.json")),
            serde_json::json!({"title": title}).to_string(),
        )
        .unwrap();
    }
    let mut indexed = open(temp.path(), BigramEncoding::Utf8Bytes, CacheMode::Memory);
    let mut direct = open(temp.path(), BigramEncoding::Utf8Bytes, CacheMode::Disabled);
    for (pattern, case_sensitive, mode) in [
        ("Müller", false, TextMode::Word),
        ("STRASSE", false, TextMode::Word),
        ("français", true, TextMode::Word),
        ("中文", true, TextMode::Word),
        ("日本語", true, TextMode::Word),
        ("مرحبا", true, TextMode::Word),
        ("😀", true, TextMode::Word),
        ("Müller|中文", true, TextMode::RegexClean),
        ("line\u{1f}break", true, TextMode::Word),
    ] {
        let query = TextQuery {
            pattern: pattern.into(),
            mode,
            case_sensitive,
            count: true,
            ..Default::default()
        };
        let expected = ids(direct
            .search_text(&query)
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.record.id));
        let actual = ids(indexed
            .search_text(&query)
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.record.id));
        assert_eq!(actual, expected, "{pattern}");
    }
    for filter in [
        "title ~ 'Müller'",
        "title ~ '中文'",
        "title = 'niño'",
        "title ~ :ignorecase('STRASSE')",
        r#"title = "M\u00fcller""#,
    ] {
        let query = JsonQuery {
            filter: filter.into(),
            count: true,
            backend: JsonQueryBackend::Auto,
            ..Default::default()
        };
        let expected = ids(direct
            .query_json(&query)
            .unwrap()
            .records
            .into_iter()
            .map(|record| record.id));
        let actual = ids(indexed
            .query_json(&query)
            .unwrap()
            .records
            .into_iter()
            .map(|record| record.id));
        assert_eq!(actual, expected, "{filter}");
    }
    let combined = TextQuery {
        pattern: "中文".into(),
        filter: Some("title ~ '中文'".into()),
        count: true,
        ..Default::default()
    };
    let expected = ids(direct
        .search_text(&combined)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.record.id));
    let actual = ids(indexed
        .search_text(&combined)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.record.id));
    assert_eq!(actual, expected);
    let cache = indexed.save_bigram_cache("docs").unwrap();
    let snapshot = objekte_core::bigram::BigramSnapshot::load(&cache).unwrap();
    assert_eq!(snapshot.filter.encoding(), BigramEncoding::Utf8Bytes);
    let mut restored = open(temp.path(), BigramEncoding::Utf8Bytes, CacheMode::Memory);
    assert!(restored.restore_cached_index_unchecked("docs").unwrap());
    let mut ascii = open(temp.path(), BigramEncoding::Ascii, CacheMode::Memory);
    assert!(!ascii.restore_cached_index_unchecked("docs").unwrap());
    ascii.warm_bigram_index("docs").unwrap();
    assert_eq!(
        objekte_core::bigram::BigramSnapshot::load(&cache)
            .unwrap()
            .filter
            .encoding(),
        BigramEncoding::Ascii
    );
}

#[test]
fn schema_override_and_unicode_secondary_cache_restore() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data/docs");
    let schemas = temp.path().join(".objekte/collections");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&schemas).unwrap();
    fs::write(
        data.join("a.json"),
        r#"{"title":"Müller","body":"**中文**"}"#,
    )
    .unwrap();
    fs::write(
        data.join("b.json"),
        r#"{"title":"Straße","body":"**日本語**"}"#,
    )
    .unwrap();
    fs::write(schemas.join("docs.schema.json"), serde_json::json!({
        "type":"object",
        "properties":{"title":{"type":"string"},"body":{"type":"string","x-full-text":"markdown"}},
        "x-objekte":{"name":"docs","path":"data/docs","type":"data","format":"json","id":"filename","bigram_encoding":"utf8_bytes"}
    }).to_string()).unwrap();
    let mut repo = open(temp.path(), BigramEncoding::Ascii, CacheMode::Memory);
    assert_eq!(
        repo.model().collections["docs"].bigram_encoding,
        Some(BigramEncoding::Utf8Bytes)
    );
    assert_eq!(
        repo.cache_status().unwrap().bigram_encodings["docs"],
        BigramEncoding::Utf8Bytes
    );
    repo.warm_bigram_index("docs").unwrap();
    repo.warm_text_index("docs").unwrap();
    let cache = repo.save_bigram_cache("docs").unwrap();
    assert_eq!(
        objekte_core::bigram::BigramSnapshot::load(&cache)
            .unwrap()
            .filter
            .encoding(),
        BigramEncoding::Utf8Bytes
    );
    let mut reopened = open(temp.path(), BigramEncoding::Ascii, CacheMode::Memory);
    reopened.warm_bigram_index("docs").unwrap();
    assert_eq!(reopened.metrics().bigram_snapshot_loads, 1);
    assert_eq!(reopened.metrics().secondary_snapshot_loads, 1);
    let query = TextQuery {
        pattern: "中文".into(),
        mode: TextMode::Word,
        count: true,
        ..Default::default()
    };
    assert_eq!(reopened.search_text(&query).unwrap().total, Some(1));

    let schema_path = schemas.join("docs.schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    schema["x-objekte"]["bigram_encoding"] = "ascii".into();
    fs::write(&schema_path, schema.to_string()).unwrap();
    let mut switched = open(temp.path(), BigramEncoding::Utf8Bytes, CacheMode::Memory);
    assert!(!switched.restore_cached_index_unchecked("docs").unwrap());
    switched.warm_bigram_index("docs").unwrap();
    assert_eq!(switched.metrics().secondary_snapshot_loads, 0);
    assert_eq!(
        objekte_core::bigram::BigramSnapshot::load(&cache)
            .unwrap()
            .filter
            .encoding(),
        BigramEncoding::Ascii
    );
}

#[test]
fn unicode_changed_file_overlay_survives_snapshot_and_restoration() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data/docs");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("a.json"), r#"{"title":"Müller"}"#).unwrap();
    fs::write(data.join("b.json"), r#"{"title":"中文"}"#).unwrap();
    let mut repo = open(temp.path(), BigramEncoding::Utf8Bytes, CacheMode::Memory);
    let cache = repo.save_bigram_cache("docs").unwrap();
    let mut repo = open(temp.path(), BigramEncoding::Utf8Bytes, CacheMode::Memory);
    repo.warm_bigram_index("docs").unwrap();
    fs::write(data.join("a.json"), r#"{"title":"Łódź"}"#).unwrap();
    repo.refresh("docs").unwrap();
    let query = TextQuery {
        pattern: "Łódź".into(),
        count: true,
        ..Default::default()
    };
    assert_eq!(repo.search_text(&query).unwrap().total, Some(1));
    let saved = objekte_core::bigram::BigramSnapshot::load(&cache).unwrap();
    assert_eq!(saved.filter.encoding(), BigramEncoding::Utf8Bytes);
    assert_eq!(saved.changed_slots.len(), 1);
    assert!(!saved.changed_bigrams.values().next().unwrap().is_empty());
    let mut reopened = open(temp.path(), BigramEncoding::Utf8Bytes, CacheMode::Memory);
    assert!(reopened.restore_cached_index_unchecked("docs").unwrap());
    assert_eq!(reopened.search_text(&query).unwrap().total, Some(1));
    fs::remove_file(data.join("b.json")).unwrap();
    fs::write(data.join("c.json"), r#"{"title":"日本語"}"#).unwrap();
    reopened.refresh("docs").unwrap();
    let deleted = TextQuery {
        pattern: "中文".into(),
        count: true,
        ..Default::default()
    };
    let created = TextQuery {
        pattern: "日本語".into(),
        count: true,
        ..Default::default()
    };
    assert_eq!(reopened.search_text(&deleted).unwrap().total, Some(0));
    assert_eq!(reopened.search_text(&created).unwrap().total, Some(1));
    reopened.reindex_bigram_cache("docs").unwrap();
    assert_eq!(
        objekte_core::bigram::BigramSnapshot::load(&cache)
            .unwrap()
            .filter
            .encoding(),
        BigramEncoding::Utf8Bytes
    );
}

#[test]
fn control_bytes_and_scalar_boundaries_are_conservative() {
    let contents = [Some("ab\u{1f}cd".as_bytes()), Some("中文".as_bytes())];
    let index = build_bigram_index_from_content_with_encoding(&contents, BigramEncoding::Utf8Bytes);
    assert!(index.query("b\u{1f}c".as_bytes()).is_none());
    let normal = index.query("中文".as_bytes()).unwrap();
    assert!(BigramFilter::is_candidate(&normal, 1));
    assert!(!BigramFilter::is_candidate(&normal, 0));
}

#[test]
fn binary_filter_rejects_unknown_encoding_and_corrupt_dimensions() {
    let contents = [Some(b"ab".as_slice()), Some(b"xy".as_slice())];
    let filter = build_bigram_index_from_content_with_encoding(&contents, BigramEncoding::Ascii);
    let bytes = filter.to_bytes().unwrap();
    let mut unknown = bytes.clone();
    unknown[9] = b'X';
    assert!(BigramFilter::from_bytes(&unknown).is_err());
    let mut lookup_size = bytes.clone();
    lookup_size[14..18].copy_from_slice(&0_u32.to_le_bytes());
    assert!(BigramFilter::from_bytes(&lookup_size).is_err());
    let mut column = bytes.clone();
    let slot = (b'a' - 32) as usize * 95 + (b'b' - 32) as usize;
    let lookup_start = 18 + 4 * 8;
    column[lookup_start + slot * 2..lookup_start + slot * 2 + 2]
        .copy_from_slice(&500_u16.to_le_bytes());
    assert!(BigramFilter::from_bytes(&column).is_err());
    let mut offsets = bytes.clone();
    let offsets_at = lookup_start
        + filter.lookup().len() * 2
        + filter.lookup().len().div_ceil(64) * 8
        + filter.dense_data().len() * 8;
    offsets[offsets_at + 4..offsets_at + 8].copy_from_slice(&1_u32.to_le_bytes());
    assert!(BigramFilter::from_bytes(&offsets).is_err());
}

#[test]
fn dense_sparse_and_legacy_ascii_roundtrips() {
    let data = (0..4096)
        .map(|slot| {
            let mut value = b"ordinary ".to_vec();
            if slot % 2 == 0 {
                value.extend_from_slice("abüö".as_bytes());
            }
            if slot % 20 == 0 {
                value.extend_from_slice("zq中文".as_bytes());
            }
            value
        })
        .collect::<Vec<_>>();
    let contents = data
        .iter()
        .map(|value| Some(value.as_slice()))
        .collect::<Vec<_>>();
    for mode in [BigramEncoding::Ascii, BigramEncoding::Utf8Bytes] {
        let filter = build_bigram_index_from_content_with_encoding(&contents, mode);
        assert!(filter.dense_count() > 0);
        assert!(filter.sparse_count() > 0);
        assert!(filter.skip_index().is_some());
        let saved = filter.to_bytes().unwrap();
        let restored = BigramFilter::from_bytes(&saved).unwrap();
        for term in ["ab", "zq", "üö", "中文"] {
            assert_eq!(
                filter.query(term.as_bytes()),
                restored.query(term.as_bytes())
            );
        }
    }
    let builder = BigramIndexBuilder::new(2);
    let skip = BigramIndexBuilder::new(2);
    builder.add_file_content(&skip, 0, b"ab");
    builder.add_file_content(&skip, 1, b"xy");
    let filter = builder.compress(None);
    let modern = filter.to_bytes().unwrap();
    let mut legacy = b"OBJBG001".to_vec();
    legacy.extend_from_slice(&modern[18..]);
    let restored = BigramFilter::from_bytes(&legacy).unwrap();
    assert_eq!(restored.encoding(), BigramEncoding::Ascii);
    assert_eq!(restored.query(b"ab"), filter.query(b"ab"));
}
