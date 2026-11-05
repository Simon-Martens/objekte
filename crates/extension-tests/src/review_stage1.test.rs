//! Regressions at the native/selected source and lifecycle boundary.

use super::*;
use objekte_core::{
    CacheMode, DerivedBackend, DerivedPredicate, DerivedQuery, Freshness, JsonQuery, Options,
    Repository, TextQuery,
    extensions::{SourceDescriptor, SourceProvider},
};
use std::{fs, path::PathBuf};


/// Reuse the fixture's secure file reader while exposing JSON bytes through a
/// registered provider rather than the concrete filesystem JSON picker.
pub(super) struct JsonAliasProvider {
    inner: MarkdownProvider,
    descriptor: Descriptor,
}


impl JsonAliasProvider {
    pub(super) fn new(root: PathBuf) -> Self {
        Self {
            inner: MarkdownProvider::new(root),
            descriptor: Descriptor::new(
                "example:json-alias",
                SemanticVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                &(),
                DataKind::Source,
                DataKind::Source,
                vec![Dependency::FileName, Dependency::SourceBytes],
            ),
        }
    }
}


impl SourceProvider for JsonAliasProvider {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn may_select(&self, collection: &str) -> bool {
        self.inner.may_select(collection)
    }

    fn enumerate(&self, collection: &str) -> Result<Vec<SourceDescriptor>> {
        let mut selected = self.inner.enumerate(collection)?;
        for source in &mut selected {
            source.provider = self.descriptor.identity.clone();
            source.format = "objekte:json".into();
        }
        Ok(selected)
    }

    fn read(&self, source: &SourceDescriptor, output: &mut Vec<u8>) -> Result<()> {
        let mut original = source.clone();
        original.provider = self.inner.descriptor().identity.clone();
        original.format = "example:markdown".into();
        self.inner.read(&original, output)
    }
}


fn year_query(backend: DerivedBackend) -> DerivedQuery {
    DerivedQuery {
        collection: "archive".into(),
        extractor: "example:publication_year".into(),
        predicate: DerivedPredicate::Ge(TypedValue::Number("1848".into())),
        sort: None,
        offset: 0,
        limit: 20,
        count: true,
        backend,
    }
}

#[test]
fn builtin_json_nodes_preserve_custom_record_structure() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("data/archive");
    fs::create_dir_all(&directory).unwrap();
    let object = r#"{"empty":[],"nested":{"vacant":{}},"map":{"0":"x"},"a/b~c":5,"publication_date":"circa 1848"}"#;
    let array = r#"[{"empty":{}},[],{"0":"x"}]"#;
    fs::write(directory.join("a.json"), object).unwrap();
    fs::write(directory.join("b.md"), object).unwrap();
    fs::write(directory.join("c.json"), array).unwrap();
    fs::write(directory.join("d.md"), array).unwrap();
    let mut repo = Repository::builder(temp.path())
        .register_source(JsonAliasProvider::new(directory))
        .unwrap()
        .register_extractor(PublicationYearExtractor::default())
        .unwrap()
        .open()
        .unwrap();
    for (native, selected) in [("a", "b"), ("c", "d")] {
        let left = repo.get_record("archive", native).unwrap().unwrap();
        let right = repo.get_record("archive", selected).unwrap().unwrap();
        assert_eq!(left.data, right.data);
    }
    for (filter, expected) in [
        ("exists(empty)", 4),
        ("exists(nested.vacant)", 2),
        ("map[\"0\"] = 'x'", 2),
        ("[\"a/b~c\"] = 5", 2),
    ] {
        let result = repo
            .query_json(&JsonQuery {
                filter: filter.into(),
                collections: Some(vec!["archive".into()]),
                limit: 20,
                count: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(result.total, Some(expected), "{filter}");
    }
    for backend in [DerivedBackend::Direct, DerivedBackend::Indexed] {
        let result = repo.query_derived(&year_query(backend)).unwrap();
        assert_eq!(result.total, Some(2));
        assert_eq!(
            result
                .records
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }
}

#[test]
fn selected_derived_policy_and_count_cap_agree() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("data/archive");
    fs::create_dir_all(&directory).unwrap();
    for id in ["a", "b", "c"] {
        fs::write(
            directory.join(format!("{id}.md")),
            r#"{"publication_date":"1848"}"#,
        )
        .unwrap();
    }
    for cache in [CacheMode::Memory, CacheMode::Disabled] {
        for freshness in [
            Freshness::MetadataOnOpen,
            Freshness::MetadataOnRequest,
            Freshness::HashOnRequest,
        ] {
            let mut repo = Repository::builder(temp.path())
                .options(Options {
                    cache,
                    freshness,
                    total_limit: 2,
                    ..Default::default()
                })
                .register_source(JsonAliasProvider::new(directory.clone()))
                .unwrap()
                .register_extractor(PublicationYearExtractor::default())
                .unwrap()
                .open()
                .unwrap();
            for backend in [DerivedBackend::Direct, DerivedBackend::Indexed] {
                let result = repo.query_derived(&year_query(backend)).unwrap();
                assert_eq!(result.total, Some(3), "{cache:?} {freshness:?} {backend:?}");
                assert!(result.truncated);
                assert_eq!(result.records.len(), 2);
            }
            assert_eq!(
                repo.derived_value_counts("archive", "example:publication_year")
                    .unwrap(),
                [(TypedValue::Number("1848".into()), 3)],
            );
        }
    }
}

#[test]
fn rebuild_observes_selected_changes_and_malformed_native_json() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("data/archive");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("a.json"), r#"{"body":"target alpha"}"#).unwrap();
    fs::write(directory.join("bad.json"), b"not JSON").unwrap();
    fs::write(directory.join("c.md"), "target gamma").unwrap();
    let mut repo = Repository::builder(temp.path())
        .register_source(MarkdownProvider::new(directory.clone()))
        .unwrap()
        .register_format(MarkdownDecoder::default())
        .unwrap()
        .open()
        .unwrap();
    let query = TextQuery {
        pattern: "target".into(),
        fields: Some(vec!["body".into()]),
        collections: Some(vec!["archive".into()]),
        count: true,
        ..Default::default()
    };
    repo.warm_text_index("archive").unwrap();
    repo.warm_bigram_index("archive").unwrap();
    assert_eq!(repo.search_text(&query).unwrap().total, Some(2));
    fs::write(directory.join("d.md"), "target delta").unwrap();
    repo.rebuild_text_index("archive").unwrap();
    assert_eq!(repo.search_text(&query).unwrap().total, Some(3));
    fs::write(directory.join("c.md"), "quiet").unwrap();
    repo.rebuild_text_index("archive").unwrap();
    assert_eq!(repo.search_text(&query).unwrap().total, Some(2));
    fs::write(directory.join("a.json"), r#"{"body":"quiet alpha"}"#).unwrap();
    let refreshed = repo.refresh("archive").unwrap();
    assert!(
        refreshed.source_reads >= 4,
        "active selected field posting was rebuilt"
    );
    let searched = repo.search_text(&query).unwrap();
    assert_eq!(searched.total, Some(1));
    assert!(
        searched.metrics.source_reads < 4,
        "refresh must retain its selected posting"
    );
}
