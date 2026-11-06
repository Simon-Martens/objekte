use super::*;

#[test]
fn query_projection_skips_unneeded_fields_and_preserves_selected_values() {
    let bytes = br#"{"ignored":{"deep":[41,42]},"needed":{"name":"found"},"other":"hidden"}"#;
    let projection = ScanProjection {
        top_level: Some(HashSet::from(["needed".to_owned()])),
        collect_text: true,
    };
    for format in [SourceFormat::Json, SourceFormat::JsonLines] {
        let selected = reader(format).parse_for_query(bytes, &projection).unwrap();
        assert_eq!(
            selected.value,
            Value::from_json(serde_json::json!({"needed":{"name":"found"}})).unwrap()
        );
        assert_eq!(selected.text.len(), 1);
        assert_eq!(selected.text[0].path, ["needed", "name"]);
        assert_eq!(selected.text[0].text, "found");
        let full = reader(format)
            .parse_for_query(bytes, &ScanProjection::full(false))
            .unwrap();
        assert!(matches!(full.value, Value::Object(ref fields) if fields.len() == 3));
        assert!(full.text.is_empty());
        assert!(
            reader(format)
                .parse_for_query(br#"{"needed":"ok","ignored":[1,]}"#, &projection)
                .is_err()
        );
    }
}

#[test]
fn standalone_jsonl_ranges_preserve_bytes_and_boundaries() {
    let bytes = "ü\n{\"x\":\"ä\"}\n\n{\"x\":\"b\"}\n".as_bytes();
    // A non-ASCII prefix is deliberately outside the valid records.
    let start = "ü\n".len();
    let valid = &bytes[start..];
    let ranges = JsonLinesReader.frame(valid).unwrap();
    assert_eq!(ranges.len(), 2);
    assert_eq!(
        &valid[ranges[0].start as usize..ranges[0].end as usize],
        "{\"x\":\"ä\"}".as_bytes()
    );
    for range in ranges {
        JsonLinesReader
            .parse(&valid[range.start as usize..range.end as usize])
            .unwrap();
    }
    assert!(JsonLinesReader.frame(b"").unwrap().is_empty());
}

#[test]
fn fused_extraction_parses_once_and_reports_bad_record() {
    let path = std::env::temp_dir().join(format!(
        "objekte-source-{}-{}.jsonl",
        std::process::id(),
        42
    ));
    std::fs::write(&path, b"{\"name\":\"A\"}\n{\"name\":\"B\"}\n").unwrap();
    let mut seen = Vec::new();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let stats = extract_file(&path, SourceFormat::JsonLines, revision, |range, record| {
        seen.push((range, record.text[0].text.clone()));
        Ok(())
    })
    .unwrap();
    assert_eq!(stats.record_parses, 2);
    assert_eq!(seen.len(), 2);
    std::fs::write(&path, b"{\"name\":\"A\"}\n{oops}\n").unwrap();
    assert!(
        extract_file(
            &path,
            SourceFormat::JsonLines,
            SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            |_, _| Ok(())
        )
        .unwrap_err()
        .contains("..")
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn nearby_multi_record_ranges_share_one_read_window() {
    let path = std::env::temp_dir().join(format!("objekte-batch-{}.jsonl", std::process::id()));
    std::fs::write(&path, b"{\"x\":1}\n{\"x\":2}\n").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let ranges = JsonLinesReader
        .frame(&std::fs::read(&path).unwrap())
        .unwrap();
    let (records, bytes_read) =
        read_record_ranges(&path, revision, &[(1, ranges[1]), (0, ranges[0])]).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].0, 0);
    assert_eq!(records[1].1, b"{\"x\":2}");
    assert_eq!(bytes_read, 15);
    let (single, single_bytes) = read_record_ranges(&path, revision, &[(1, ranges[1])]).unwrap();
    assert_eq!(single, vec![(1, b"{\"x\":2}".to_vec())]);
    assert_eq!(single_bytes, 7);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn changed_source_rejects_old_ranges_and_incomplete_extraction() {
    let path = std::env::temp_dir().join(format!("objekte-stale-{}.jsonl", std::process::id()));
    std::fs::write(&path, b"{\"x\":1}\n{\"x\":2}\n").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let ranges = JsonLinesReader
        .frame(&std::fs::read(&path).unwrap())
        .unwrap();
    let changed = extract_file(&path, SourceFormat::JsonLines, revision, |_, _| {
        std::fs::write(&path, b"{\"x\":1}\n{\"x\":22}\n").map_err(|e| e.to_string())?;
        Ok(())
    });
    assert!(changed.unwrap_err().contains("stale source revision"));
    assert!(
        read_record_range(&path, revision, ranges[1])
            .unwrap_err()
            .contains("stale source revision")
    );
    let current = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    assert!(
        read_record_ranges(
            &path,
            current,
            &[(
                0,
                ByteRange {
                    start: 0,
                    end: current.size + 1
                }
            )]
        )
        .unwrap_err()
        .contains("invalid byte range")
    );
    std::fs::remove_file(path).unwrap();
}
