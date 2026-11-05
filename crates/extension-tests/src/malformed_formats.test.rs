//! Public consumer example: a small UTF-8 article format with recoverable syntax errors.

use super::{MarkdownProvider, RecordAccess};
use objekte_core::{
    CacheMode, Error, JsonPath, JsonPathStep, JsonQuery, JsonQueryBackend, Options, RecordStatus,
    Repository, Result, SourceSpan, TextMode, TextQuery,
    extensions::{
        BindingContext, DataKind, DecodeOutcome, DecodedRecord, Dependency, Descriptor, Extraction,
        FormatDecoder, FormatDiagnostic, ScalarValue, SemanticVersion, SourceDescriptor,
        TypedValue, ValueExtractor,
    },
};
use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
};


struct Article<'a> {
    year: &'a str,
    body: &'a str,
}


impl Article<'_> {
    fn year_value(&self) -> ScalarValue<'_> {
        if self.year == "unknown" {
            ScalarValue::Text(self.year)
        } else {
            ScalarValue::Number(self.year)
        }
    }
}


impl RecordAccess for Article<'_> {
    fn path_present(&self, path: &JsonPath) -> Result<bool> {
        Ok(matches!(path.as_slice(), [JsonPathStep::Key(key)] if key == "year" || key == "body"))
    }

    fn visit_path(
        &self,
        path: &JsonPath,
        visit: &mut dyn FnMut(ScalarValue<'_>) -> Result<()>,
    ) -> Result<usize> {
        match path.as_slice() {
            [JsonPathStep::Key(key)] if key == "year" => visit(self.year_value())?,
            [JsonPathStep::Key(key)] if key == "body" => visit(ScalarValue::Text(self.body))?,
            _ => return Ok(0),
        }
        Ok(1)
    }
}


impl DecodedRecord for Article<'_> {
    fn visit_leaves(
        &self,
        visit: &mut dyn FnMut(&str, ScalarValue<'_>) -> Result<()>,
    ) -> Result<()> {
        visit("/year", self.year_value())?;
        visit("/body", ScalarValue::Text(self.body))
    }
}


/// The provider assigns identity from the filename. A valid article starts
/// `# YYYY\n\n`; malformed UTF-8 has no text fallback. Successful UTF-8
/// fallback borrows the original bytes and preserves markup literally.
struct ArticleDecoder {
    descriptor: Descriptor,
    fallback: bool,
}


impl ArticleDecoder {
    fn new(fallback: bool) -> Self {
        Self {
            descriptor: Descriptor::new(
                "example:markdown",
                SemanticVersion {
                    major: 2,
                    minor: 0,
                    patch: 0,
                },
                &fallback,
                DataKind::Source,
                DataKind::Record,
                vec![Dependency::SourceBytes],
            ),
            fallback,
        }
    }
}


impl FormatDecoder for ArticleDecoder {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn accepts(&self, source: &SourceDescriptor) -> bool {
        source.format == self.descriptor.identity
    }

    fn decode(
        &self,
        bytes: &[u8],
        visit: &mut dyn FnMut(&dyn DecodedRecord) -> Result<()>,
    ) -> Result<()> {
        match self.decode_outcome(bytes, visit)? {
            DecodeOutcome::Decoded => Ok(()),
            DecodeOutcome::Malformed(diagnostics) => Err(Error::MalformedSource {
                format: self.descriptor.identity.clone(),
                message: diagnostics[0].message.clone(),
            }),
        }
    }

    fn decode_outcome(
        &self,
        bytes: &[u8],
        visit: &mut dyn FnMut(&dyn DecodedRecord) -> Result<()>,
    ) -> Result<DecodeOutcome> {
        if bytes == b"!operational" {
            return Err(Error::Request("article decoder failed".into()));
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            return Ok(DecodeOutcome::Malformed(vec![FormatDiagnostic::new(
                "article_encoding",
                "article must be UTF-8",
            )]));
        };
        if text == "!zero" {
            return Ok(DecodeOutcome::Decoded);
        }
        let demonstration = Article {
            year: "1848",
            body: "example",
        };
        if text == "!twice" {
            visit(&demonstration)?;
            visit(&demonstration)?;
            return Ok(DecodeOutcome::Decoded);
        }
        if text == "!late-malformed" {
            visit(&demonstration)?;
            return Ok(DecodeOutcome::Malformed(vec![FormatDiagnostic::new(
                "article_syntax",
                "late syntax error",
            )]));
        }
        if text == "!hide-callback" {
            let _ = visit(&demonstration);
            return Ok(DecodeOutcome::Malformed(vec![FormatDiagnostic::new(
                "article_syntax",
                "hidden failure",
            )]));
        }
        if text == "!hide-success" {
            let _ = visit(&demonstration);
            return Ok(DecodeOutcome::Decoded);
        }
        if text == "!bad-span" {
            return Ok(DecodeOutcome::Malformed(vec![FormatDiagnostic {
                code: "article_syntax".into(),
                message: "invalid source range".into(),
                span: Some(SourceSpan {
                    start: 0,
                    end: bytes.len() + 1,
                }),
            }]));
        }
        if text == "!empty-diagnostics" {
            return Ok(DecodeOutcome::Malformed(Vec::new()));
        }
        if let Some((header, body)) = text.split_once("\n\n")
            && let Some(year) = header.strip_prefix("# ")
            && (year == "unknown"
                || year.len() == 4 && year.bytes().all(|byte| byte.is_ascii_digit()))
        {
            visit(&Article { year, body })?;
            return Ok(DecodeOutcome::Decoded);
        }
        Ok(DecodeOutcome::Malformed(vec![FormatDiagnostic {
            code: "article_syntax".into(),
            message: "expected '# YYYY' followed by a blank line".into(),
            span: Some(SourceSpan {
                start: 0,
                end: bytes.len().min(2),
            }),
        }]))
    }

    fn malformed_text<'a>(&self, bytes: &'a [u8]) -> Result<Option<Cow<'a, str>>> {
        if bytes == b"!fallback-error" {
            return Err(Error::Request("fallback sentinel".into()));
        }
        if !self.fallback {
            return Ok(None);
        }
        Ok(std::str::from_utf8(bytes).ok().map(Cow::Borrowed))
    }
}


fn repository(root: &Path, fallback: bool) -> Repository {
    Repository::builder(root)
        .register_source(MarkdownProvider::new(root.join("data/articles")))
        .unwrap()
        .register_format(ArticleDecoder::new(fallback))
        .unwrap()
        .open()
        .unwrap()
}

fn fixture(schema: bool) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("data/articles");
    fs::create_dir_all(path.join(".editor")).unwrap();
    if schema {
        fs::write(path.join(".editor/schema.json"), r#"{"type":"object","required":["year","body"],"properties":{"year":{"type":"integer","index":true},"body":{"type":"string","x-full-text":"plain"}}}"#).unwrap();
    }
    fs::write(path.join("valid.md"), b"# 1848\n\nQuiet archive").unwrap();
    fs::write(path.join("broken.md"), b"Broken <em>Moon</em> & rough").unwrap();
    (temp, path)
}

#[test]
fn malformed_record_is_retained_and_searches_only_unscoped_raw_text() {
    let (temp, path) = fixture(true);
    let mut repo = repository(temp.path(), true);
    repo.warm_bigram_index("articles").unwrap();
    assert_eq!(repo.validate_collection("articles").unwrap().validated, 1);
    let broken = repo.get_record("articles", "broken").unwrap().unwrap();
    assert_eq!(broken.state.status, RecordStatus::IncorrectFormat);
    assert_eq!(
        broken.raw.as_deref(),
        Some(b"Broken <em>Moon</em> & rough".as_slice())
    );
    assert_eq!(broken.state.issues[0].code, "article_syntax");
    assert_eq!(broken.state.issues[0].location, "");
    assert_eq!(
        broken.state.issues[0].source.as_ref().unwrap().format,
        "example:markdown"
    );
    assert_eq!(
        broken.state.issues[0].source.as_ref().unwrap().span,
        Some(SourceSpan { start: 0, end: 2 })
    );
    assert_eq!(
        serde_json::to_value(broken.state.status).unwrap(),
        "incorrect_format"
    );
    let serialized_issue = serde_json::to_value(&broken.state.issues[0]).unwrap();
    assert_eq!(serialized_issue["source"]["span"]["end"], 2);
    assert!(
        repo.with_source_record(
            &repo
                .source_descriptors("articles")
                .unwrap()
                .into_iter()
                .find(|source| source.record_id == "broken")
                .unwrap(),
            |_| Ok(())
        )
        .is_err()
    );
    for (pattern, mode) in [
        ("Moon", TextMode::Word),
        ("<em>Moon</em>", TextMode::WordRaw),
        ("<em>Moon</em>", TextMode::Regex),
    ] {
        let query = TextQuery {
            pattern: pattern.into(),
            mode,
            collections: Some(vec!["articles".into()]),
            count: true,
            ..Default::default()
        };
        let result = repo.search_text(&query).unwrap();
        assert_eq!(result.total, Some(1), "{pattern:?}");
        assert_eq!(result.hits[0].record.id, "broken");
        assert_eq!(result.hits[0].fields, ["$raw"]);
        assert_eq!(result.hits[0].matched_paths, ["$raw"]);
        let scoped = repo
            .search_text(&TextQuery {
                fields: Some(vec!["body".into()]),
                ..query
            })
            .unwrap();
        assert_eq!(scoped.total, Some(0));
    }
    for (filter, expected) in [
        ("@status = 'incorrect_format'", vec!["broken"]),
        ("year = 1848", vec!["valid"]),
        (
            "@status = 'incorrect_format' || year = 1848",
            vec!["broken", "valid"],
        ),
    ] {
        let result = repo
            .query_json(&JsonQuery {
                filter: filter.into(),
                collections: Some(vec!["articles".into()]),
                count: true,
                ..Default::default()
            })
            .unwrap();
        let ids = result
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, expected, "{filter}");
    }
    fs::write(path.join("broken.md"), b"# 1850\n\nMoon repaired").unwrap();
    repo.refresh("articles").unwrap();
    let repaired = repo.get_record("articles", "broken").unwrap().unwrap();
    assert_eq!(repaired.state.status, RecordStatus::Pending);
    assert_eq!(repaired.data["year"], 1850);
    assert!(repaired.raw.is_none());
    let validated = repo
        .query_json(&JsonQuery {
            filter: "year = 1850".into(),
            collections: Some(vec!["articles".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(validated.records[0].state.status, RecordStatus::Valid);
    fs::write(
        path.join("invalid.md"),
        b"# unknown\n\nMoon logical content",
    )
    .unwrap();
    repo.refresh("articles").unwrap();
    let invalid = repo
        .query_json(&JsonQuery {
            filter: "year = 'unknown'".into(),
            collections: Some(vec!["articles".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(invalid.records[0].state.status, RecordStatus::Invalid);
    assert!(
        invalid.records[0]
            .state
            .issues
            .iter()
            .any(|issue| issue.code == "schema")
    );
    assert!(invalid.records[0].raw.is_none());
}

#[test]
fn builtin_json_through_a_provider_keeps_native_malformed_status_and_raw_search() {
    use super::review_stage1::JsonAliasProvider;
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("data/mixed");
    fs::create_dir_all(&directory).unwrap();
    for (name, bytes) in [
        ("good.json", b"null".as_slice()),
        ("good_alias.md", b"null".as_slice()),
        ("array.json", b"[1,2]".as_slice()),
        ("array_alias.md", b"[1,2]".as_slice()),
        ("bad.json", br#"{"x":"M\u006fnd", broken"#.as_slice()),
        ("bad_alias.md", br#"{"x":"M\u006fnd", broken"#.as_slice()),
        ("bytes.json", b"bad \xff Moon".as_slice()),
        ("bytes_alias.md", b"bad \xff Moon".as_slice()),
    ] {
        fs::write(directory.join(name), bytes).unwrap();
    }
    let mut repo = Repository::builder(temp.path())
        .register_source(JsonAliasProvider::new(directory))
        .unwrap()
        .open()
        .unwrap();
    repo.warm_bigram_index("mixed").unwrap();
    for (native, alias) in [
        ("good", "good_alias"),
        ("array", "array_alias"),
        ("bad", "bad_alias"),
        ("bytes", "bytes_alias"),
    ] {
        let left = repo.get_record("mixed", native).unwrap().unwrap();
        let right = repo.get_record("mixed", alias).unwrap().unwrap();
        assert_eq!(left.state.status, right.state.status);
        assert_eq!(left.data, right.data);
        assert_eq!(left.raw, right.raw);
        if right.state.status.is_malformed() {
            assert_eq!(right.state.status, RecordStatus::IncorrectJson);
            assert_eq!(
                serde_json::to_value(right.state.status).unwrap(),
                "incorrect_json"
            );
            assert_eq!(right.state.issues[0].code, "json_syntax");
            assert_eq!(
                right.state.issues[0].source.as_ref().unwrap().format,
                "objekte:json"
            );
        }
    }
    for pattern in ["Mond", "Moon"] {
        let result = repo
            .search_text(&TextQuery {
                pattern: pattern.into(),
                collections: Some(vec!["mixed".into()]),
                count: true,
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(result.total, Some(2), "{pattern}");
        assert!(
            result
                .hits
                .iter()
                .all(|hit| hit.record.state.status == RecordStatus::IncorrectJson)
        );
        assert!(result.hits.iter().all(|hit| hit.fields == ["$raw"]));
    }
}

#[test]
fn refresh_and_restore_replace_logical_postings_with_malformed_membership() {
    let (temp, path) = fixture(true);
    for index in 0..64 {
        fs::write(
            path.join(format!("n{index:03}.json")),
            format!(r#"{{"year":1850,"body":"ordinary filler {index}"}}"#),
        )
        .unwrap();
    }
    let mut repo = repository(temp.path(), true);
    repo.warm_bigram_index("articles").unwrap();
    let year = JsonQuery {
        filter: "year = 1848".into(),
        collections: Some(vec!["articles".into()]),
        count: true,
        ..Default::default()
    };
    let before = repo.query_json(&year).unwrap();
    assert_eq!(
        before
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        ["valid"]
    );
    assert!(
        before.metrics.source_reads < 10,
        "rare scalar term should keep its posting"
    );
    fs::write(path.join("valid.md"), b"Broken Moon <b>first</b>").unwrap();
    let refreshed = repo.refresh("articles").unwrap();
    assert!(
        refreshed.source_reads < 10,
        "only changed selected sources should decode"
    );
    assert!(
        refreshed.parses <= refreshed.source_reads,
        "refresh must not parse a changed source twice"
    );
    assert_eq!(repo.query_json(&year).unwrap().total, Some(0));
    let raw = TextQuery {
        pattern: "Moon".into(),
        collections: Some(vec!["articles".into()]),
        count: true,
        ..Default::default()
    };
    let first = repo.search_text(&raw).unwrap();
    assert_eq!(first.total, Some(2));
    assert!(
        first
            .hits
            .iter()
            .any(|hit| hit.record.id == "valid" && hit.fields == ["$raw"])
    );
    for offset in 0..=2 {
        let page = repo
            .search_text(&TextQuery {
                offset,
                limit: 1,
                ..raw.clone()
            })
            .unwrap();
        assert_eq!(page.total, Some(2));
        assert_eq!(
            page.hits
                .iter()
                .map(|hit| &hit.record.id)
                .collect::<Vec<_>>(),
            first
                .hits
                .iter()
                .skip(offset)
                .take(1)
                .map(|hit| &hit.record.id)
                .collect::<Vec<_>>()
        );
    }
    repo.save_bigram_cache("articles").unwrap();
    drop(repo);
    let mut reopened = repository(temp.path(), true);
    let restored = reopened.search_text(&raw).unwrap();
    assert_eq!(restored.total, Some(2));
    assert!(
        restored
            .hits
            .iter()
            .all(|hit| hit.record.state.status == RecordStatus::IncorrectFormat)
    );
    fs::write(path.join("valid.md"), b"Broken Moon <b>second</b>").unwrap();
    reopened.refresh("articles").unwrap();
    assert_eq!(
        reopened
            .search_text(&TextQuery {
                pattern: "first".into(),
                ..raw.clone()
            })
            .unwrap()
            .total,
        Some(0)
    );
    assert_eq!(
        reopened
            .search_text(&TextQuery {
                pattern: "second".into(),
                ..raw.clone()
            })
            .unwrap()
            .total,
        Some(1)
    );
    fs::write(path.join("valid.md"), b"# 1848\n\nMoon repaired").unwrap();
    reopened.refresh("articles").unwrap();
    assert_eq!(reopened.query_json(&year).unwrap().total, Some(1));
    let repaired = reopened.search_text(&raw).unwrap();
    assert_eq!(repaired.total, Some(2));
    assert!(
        repaired
            .hits
            .iter()
            .any(|hit| hit.record.id == "valid" && hit.fields == ["body"])
    );
    fs::rename(path.join("valid.md"), path.join("renamed.md")).unwrap();
    reopened.refresh("articles").unwrap();
    assert_eq!(reopened.query_json(&year).unwrap().records[0].id, "renamed");
    fs::remove_file(path.join("renamed.md")).unwrap();
    reopened.refresh("articles").unwrap();
    assert_eq!(reopened.query_json(&year).unwrap().total, Some(0));
}

#[test]
fn malformed_stream_callback_error_stops_and_does_not_complete_cache_entry() {
    let (temp, path) = fixture(true);
    fs::write(path.join("second.md"), b"Broken Moon second").unwrap();
    let mut repo = repository(temp.path(), true);
    let query = TextQuery {
        pattern: "Moon".into(),
        collections: Some(vec!["articles".into()]),
        count: true,
        ..Default::default()
    };
    let error = repo
        .search_text_stream(&query, |_, _| Err(Error::Request("stream sentinel".into())))
        .unwrap_err();
    assert!(error.to_string().contains("stream sentinel"));
    let complete = repo.search_text(&query).unwrap();
    assert_eq!(complete.total, Some(2));
    assert_eq!(complete.metrics.ordered_result_hits, 0);
    assert!(complete.hits.iter().all(|hit| hit.fields == ["$raw"]));
}

#[test]
fn indexed_and_direct_boolean_metadata_queries_agree_with_malformed_sources() {
    let (temp, path) = fixture(true);
    fs::write(
        path.join("native.json"),
        r#"{"year":1848,"body":"native text"}"#,
    )
    .unwrap();
    fs::write(path.join("native_bad.json"), b"broken native JSON").unwrap();
    let mut indexed = repository(temp.path(), true);
    indexed.warm_bigram_index("articles").unwrap();
    let mut direct = Repository::builder(temp.path())
        .options(Options {
            cache: CacheMode::Disabled,
            query_cache_bytes: 0,
            ..Default::default()
        })
        .register_source(MarkdownProvider::new(path))
        .unwrap()
        .register_format(ArticleDecoder::new(true))
        .unwrap()
        .open()
        .unwrap();
    for filter in [
        "@status = 'incorrect_format'",
        "@status = 'incorrect_json'",
        "year = 1848",
        "missing(year)",
        "@status = 'incorrect_format' || year = 1848",
        "@status = 'incorrect_format' || missing(year)",
        "@status != 'incorrect_format' && year = 1848",
        "@status = 'incorrect_format' && year = 1848",
    ] {
        let query = JsonQuery {
            filter: filter.into(),
            collections: Some(vec!["articles".into()]),
            count: true,
            limit: 1,
            ..Default::default()
        };
        let fast = indexed.query_json(&query).unwrap();
        let exact = direct
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..query.clone()
            })
            .unwrap();
        assert_eq!(fast.total, exact.total, "{filter}");
        assert_eq!(fast.truncated, exact.truncated, "{filter}");
        assert_eq!(
            fast.records
                .iter()
                .map(|record| &record.id)
                .collect::<Vec<_>>(),
            exact
                .records
                .iter()
                .map(|record| &record.id)
                .collect::<Vec<_>>(),
            "{filter}"
        );
    }
}

#[test]
fn failed_fallback_and_decoder_refresh_do_not_publish_a_completed_result() {
    let (temp, path) = fixture(true);
    let mut repo = repository(temp.path(), true);
    repo.warm_bigram_index("articles").unwrap();
    fs::write(path.join("fail.md"), b"!operational").unwrap();
    assert!(
        repo.refresh("articles")
            .unwrap_err()
            .to_string()
            .contains("article decoder failed")
    );
    fs::write(path.join("fail.md"), b"!fallback-error").unwrap();
    repo.refresh("articles").unwrap();
    let query = TextQuery {
        pattern: "fallback".into(),
        collections: Some(vec!["articles".into()]),
        count: true,
        ..Default::default()
    };
    assert!(
        repo.search_text(&query)
            .unwrap_err()
            .to_string()
            .contains("fallback sentinel")
    );
    fs::write(path.join("fail.md"), b"# 1848\n\nworking content").unwrap();
    repo.refresh("articles").unwrap();
    assert_eq!(repo.search_text(&query).unwrap().total, Some(0));
    let valid = repo
        .search_text(&TextQuery {
            pattern: "working".into(),
            ..query
        })
        .unwrap();
    assert_eq!(valid.total, Some(1));
    assert_eq!(valid.hits[0].record.id, "fail");
}


struct GuardedYear(Descriptor);


impl Default for GuardedYear {
    fn default() -> Self {
        Self(Descriptor::new(
            "example:guarded-year",
            SemanticVersion {
                major: 1,
                minor: 0,
                patch: 0,
            },
            &(),
            DataKind::Record,
            DataKind::TypedValues,
            vec![Dependency::SourceBytes],
        ))
    }
}


impl ValueExtractor for GuardedYear {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }

    fn binds(&self, _: &BindingContext<'_>) -> bool {
        true
    }

    fn extract(
        &self,
        record: &dyn RecordAccess,
        output: &mut Vec<TypedValue>,
    ) -> Result<Extraction> {
        let year = vec![JsonPathStep::Key("year".into())];
        let body = vec![JsonPathStep::Key("body".into())];
        if !record.path_present(&year)? && !record.path_present(&body)? {
            return Err(Error::Request(
                "extractor saw a malformed placeholder".into(),
            ));
        }
        let present = record.path_present(&year)?;
        record.visit_path(&year, &mut |value| {
            if let ScalarValue::Number(number) = value {
                output.push(TypedValue::Number(number.into()));
            }
            Ok(())
        })?;
        Ok(if present {
            Extraction::Present
        } else {
            Extraction::Missing
        })
    }
}


#[test]
fn derived_direct_and_indexed_paths_skip_malformed_logical_input() {
    use objekte_core::{DerivedBackend, DerivedPredicate, DerivedQuery};
    let (temp, path) = fixture(true);
    let open = || {
        Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(path.clone()))
            .unwrap()
            .register_format(ArticleDecoder::new(true))
            .unwrap()
            .register_extractor(GuardedYear::default())
            .unwrap()
            .open()
            .unwrap()
    };
    let mut repo = open();
    repo.build_derived_posting("articles", "example:guarded-year")
        .unwrap();
    for backend in [DerivedBackend::Direct, DerivedBackend::Indexed] {
        let result = repo
            .query_derived(&DerivedQuery {
                collection: "articles".into(),
                extractor: "example:guarded-year".into(),
                predicate: DerivedPredicate::Eq(TypedValue::Number("1848".into())),
                sort: None,
                offset: 0,
                limit: 10,
                count: true,
                backend,
            })
            .unwrap();
        assert_eq!(result.total, Some(1));
        assert_eq!(result.records[0].id, "valid");
    }
    fs::write(path.join("valid.md"), b"no article header").unwrap();
    repo.refresh("articles").unwrap();
    for backend in [DerivedBackend::Direct, DerivedBackend::Indexed] {
        assert_eq!(
            repo.query_derived(&DerivedQuery {
                collection: "articles".into(),
                extractor: "example:guarded-year".into(),
                predicate: DerivedPredicate::Eq(TypedValue::Number("1848".into())),
                sort: None,
                offset: 0,
                limit: 10,
                count: true,
                backend,
            })
            .unwrap()
            .total,
            Some(0)
        );
    }
    repo.save_bigram_cache("articles").unwrap();
    drop(repo);
    let mut restored = open();
    assert_eq!(
        restored
            .query_derived(&DerivedQuery {
                collection: "articles".into(),
                extractor: "example:guarded-year".into(),
                predicate: DerivedPredicate::AnyValue,
                sort: None,
                offset: 0,
                limit: 10,
                count: true,
                backend: DerivedBackend::Indexed,
            })
            .unwrap()
            .total,
        Some(0)
    );
    fs::write(path.join("valid.md"), b"# 1848\n\nrepaired article").unwrap();
    restored.refresh("articles").unwrap();
    assert_eq!(
        restored
            .query_derived(&DerivedQuery {
                collection: "articles".into(),
                extractor: "example:guarded-year".into(),
                predicate: DerivedPredicate::AnyValue,
                sort: None,
                offset: 0,
                limit: 10,
                count: true,
                backend: DerivedBackend::Indexed,
            })
            .unwrap()
            .total,
        Some(1)
    );
}

#[test]
fn selected_text_snapshot_retains_malformed_membership_and_rejects_old_metadata() {
    let (temp, path) = fixture(true);
    let snapshot = temp.path().join("article-body.snapshot");
    let open = || {
        Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(path.clone()))
            .unwrap()
            .register_format(ArticleDecoder::new(true))
            .unwrap()
            .open()
            .unwrap()
    };
    let mut repo = open();
    repo.build_representation_index("articles", "objekte:stored", "body")
        .unwrap();
    repo.save_selected_representation_snapshot("articles", "objekte:stored", "body", &snapshot)
        .unwrap();
    drop(repo);
    let mut restored = open();
    assert!(
        restored
            .restore_selected_representation_snapshot(
                "articles",
                "objekte:stored",
                "body",
                &snapshot
            )
            .unwrap()
    );
    let mut changed_policy = repository(temp.path(), false);
    assert!(
        !changed_policy
            .restore_selected_representation_snapshot(
                "articles",
                "objekte:stored",
                "body",
                &snapshot
            )
            .unwrap()
    );
    let mut companion = snapshot.as_os_str().to_os_string();
    companion.push(".selected-meta");
    let companion = PathBuf::from(companion);
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&companion).unwrap()).unwrap();
    assert_eq!(metadata["version"], 6);
    assert_eq!(metadata["malformed_slots"].as_array().unwrap().len(), 1);
    metadata["version"] = 5.into();
    fs::write(&companion, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let mut incompatible = open();
    assert!(
        !incompatible
            .restore_selected_representation_snapshot(
                "articles",
                "objekte:stored",
                "body",
                &snapshot
            )
            .unwrap()
    );
}

#[test]
fn no_schema_and_disabled_or_invalid_encoding_fallback_remain_distinct() {
    let (temp, path) = fixture(false);
    let mut repo = repository(temp.path(), false);
    let broken = repo.get_record("articles", "broken").unwrap().unwrap();
    assert_eq!(broken.state.status, RecordStatus::IncorrectFormat);
    assert_eq!(
        repo.get_record("articles", "valid")
            .unwrap()
            .unwrap()
            .state
            .status,
        RecordStatus::NoSchema
    );
    assert_eq!(
        repo.search_text(&TextQuery {
            pattern: "Moon".into(),
            collections: Some(vec!["articles".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap()
        .total,
        Some(0)
    );
    fs::write(path.join("invalid.md"), b"bad \xff Moon").unwrap();
    repo.refresh("articles").unwrap();
    assert_eq!(
        repo.get_record("articles", "invalid")
            .unwrap()
            .unwrap()
            .state
            .issues[0]
            .code,
        "article_encoding"
    );
    assert_eq!(
        repo.search_text(&TextQuery {
            pattern: "Moon".into(),
            collections: Some(vec!["articles".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap()
        .total,
        Some(0)
    );
}

#[test]
fn decoder_contract_and_operational_failures_remain_errors() {
    let (temp, path) = fixture(false);
    let repo = repository(temp.path(), true);
    let descriptor = repo
        .source_descriptors("articles")
        .unwrap()
        .into_iter()
        .find(|source| source.record_id == "valid")
        .unwrap();
    let callback = repo
        .with_source_record(&descriptor, |_| {
            Err::<(), _>(Error::Request("callback sentinel".into()))
        })
        .unwrap_err();
    assert!(callback.to_string().contains("callback sentinel"));
    for (name, content, expected) in [
        ("zero", "!zero", "no record"),
        ("twice", "!twice", "multiple records"),
        (
            "late",
            "!late-malformed",
            "malformed content after emitting",
        ),
        ("hidden", "!hide-callback", "callback sentinel"),
        ("hidden_success", "!hide-success", "callback sentinel"),
        ("bad_span", "!bad-span", "outside source bytes"),
        (
            "empty_diagnostics",
            "!empty-diagnostics",
            "source syntax is malformed",
        ),
        ("operation", "!operational", "article decoder failed"),
    ] {
        fs::write(path.join(format!("{name}.md")), content).unwrap();
        let descriptor = repo
            .source_descriptors("articles")
            .unwrap()
            .into_iter()
            .find(|source| source.record_id == name)
            .unwrap();
        let result = repo.with_source_record(&descriptor, |_| {
            Err::<(), _>(Error::Request("callback sentinel".into()))
        });
        let error = result.unwrap_err().to_string();
        if name == "twice" || name == "late" || name == "bad_span" {
            // Use a successful callback to expose the decoder's own violation.
            let error = repo
                .with_source_record(&descriptor, |_| Ok(()))
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{name}: {error}");
        } else {
            assert!(error.contains(expected), "{name}: {error}");
        }
    }
    let mut inspected = repository(temp.path(), true);
    let generic = inspected
        .get_record("articles", "empty_diagnostics")
        .unwrap()
        .unwrap();
    assert_eq!(generic.state.status, RecordStatus::IncorrectFormat);
    assert_eq!(generic.state.issues[0].code, "format_syntax");
}
