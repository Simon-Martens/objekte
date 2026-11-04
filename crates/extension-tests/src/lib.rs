//! Downstream compilation and behavior checks using only public core APIs.

use objekte_core::{
    Error, Result,
    extensions::{
        BindingContext, CandidateConstraint, CompiledOperator, DataKind, Dependency, Descriptor,
        QueryOperator, ScalarValue, SemanticVersion, TextRepresentation,
    },
};
use serde_json::Value;


/// Exact, case-sensitive prefix comparison compiled once per request.
pub struct PrefixOperator {
    descriptor: Descriptor,
}


impl Default for PrefixOperator {
    fn default() -> Self {
        Self {
            descriptor: Descriptor::new(
                "example:prefix",
                SemanticVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                &(),
                DataKind::Text,
                DataKind::Predicate,
                vec![Dependency::SourceBytes],
            ),
        }
    }
}


struct Prefix {
    prefix: String,
}


impl CompiledOperator for Prefix {
    fn matches(&self, value: ScalarValue<'_>, _: &mut String) -> Result<bool> {
        Ok(matches!(value, ScalarValue::Text(text) if text.starts_with(&self.prefix)))
    }

    fn candidate(&self) -> CandidateConstraint {
        CandidateConstraint::BigramLiteral {
            literal: self.prefix.clone(),
            representation: "objekte:stored".into(),
        }
    }
}


impl QueryOperator for PrefixOperator {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn compile(&self, parameters: &Value) -> Result<Box<dyn CompiledOperator>> {
        let prefix = parameters
            .as_str()
            .ok_or_else(|| Error::Request("prefix must be a string".into()))?;
        Ok(Box::new(Prefix {
            prefix: prefix.into(),
        }))
    }
}


/// Independent representation: ASCII whitespace is removed at the ends only.
pub struct TrimmedRepresentation {
    descriptor: Descriptor,
}


impl Default for TrimmedRepresentation {
    fn default() -> Self {
        Self {
            descriptor: Descriptor::new(
                "example:trimmed",
                SemanticVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                &(),
                DataKind::Text,
                DataKind::Text,
                vec![
                    Dependency::SourceBytes,
                    Dependency::Schema("x-trimmed".into()),
                ],
            ),
        }
    }
}


impl TextRepresentation for TrimmedRepresentation {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn binds(&self, context: &BindingContext<'_>) -> bool {
        context.path == "body"
            && context
                .schema
                .and_then(|schema| schema.pointer("/properties/body/x-trimmed"))
                .and_then(Value::as_bool)
                == Some(true)
    }

    fn render<'a>(&self, value: &'a str, _: &'a mut String) -> Result<Option<&'a str>> {
        Ok(Some(value.trim_ascii()))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use objekte_core::{
        CacheMode, Options, Repository, TextFormat, TextMode, TextQuery,
        extensions::{NativeTextOperator, RenderedRepresentation, StoredRepresentation},
    };
    use std::fs;

    #[test]
    fn prefix_uses_registered_representation_and_engine_results() {
        let root = tempfile::tempdir().unwrap();
        let books = root.path().join("data/books");
        fs::create_dir_all(books.join(".editor")).unwrap();
        fs::write(
            books.join(".editor/schema.json"),
            r#"{"type":"object","properties":{"body":{"type":"string","x-trimmed":true},"html":{"type":"string","x-full-text":"html"},"md":{"type":"string","x-full-text":"markdown"},"xml":{"type":"string","x-full-text":"xml"}}}"#,
        ).unwrap();
        fs::write(
            books.join("a.json"),
            r#"{"body":"  Hello A  ","html":"He<b>llo</b>","md":"**He**lp","xml":"<root>He<em>lp</em></root>"}"#,
        )
        .unwrap();
        fs::write(
            books.join("b.json"),
            r#"{"body":"Hello B","html":"<i>Other</i>","md":"Straße","xml":"<root><p>broken"}"#,
        )
        .unwrap();
        fs::write(
            books.join("c.json"),
            r#"{"body":"Other","html":"<i>Other</i>"}"#,
        )
        .unwrap();
        let mut repository = Repository::builder(root.path())
            .register_representation(StoredRepresentation::default())
            .unwrap()
            .register_representation(TrimmedRepresentation::default())
            .unwrap()
            .register_representation(RenderedRepresentation::new(TextFormat::Html).unwrap())
            .unwrap()
            .register_representation(RenderedRepresentation::new(TextFormat::Markdown).unwrap())
            .unwrap()
            .register_representation(RenderedRepresentation::new(TextFormat::Xml).unwrap())
            .unwrap()
            .register_operator(PrefixOperator::default())
            .unwrap()
            .register_operator(NativeTextOperator::new(TextMode::Word, false))
            .unwrap()
            .register_operator(NativeTextOperator::new(TextMode::WordRaw, false))
            .unwrap()
            .register_operator(NativeTextOperator::new(TextMode::Regex, false))
            .unwrap()
            .open()
            .unwrap();
        let mut request = TextQuery {
            pattern: "Hello".into(),
            collections: Some(vec!["books".into()]),
            fields: Some(vec!["body".into()]),
            count: true,
            ..Default::default()
        };
        let trimmed = repository
            .search_text_with(&request, "example:trimmed", "example:prefix")
            .unwrap();
        assert_eq!(trimmed.total, Some(2));
        assert_eq!(trimmed.hits.len(), 2);
        assert!(trimmed.hits.iter().all(|hit| hit.fields == ["body"]));

        request.fields = Some(vec!["html".into()]);
        let rendered = repository
            .search_text_with(&request, "objekte:visible-html", "example:prefix")
            .unwrap();
        assert_eq!(rendered.total, Some(1));
        assert_eq!(rendered.hits[0].record.id, "a");
        assert_eq!(rendered.hits[0].matched_paths, ["html"]);

        for (mode, pattern, representation, operator) in [
            (
                TextMode::Word,
                "Hello",
                "objekte:visible-html",
                "objekte:word",
            ),
            (
                TextMode::WordRaw,
                "He<b>",
                "objekte:stored",
                "objekte:word-raw",
            ),
            (
                TextMode::Regex,
                "He<b>llo",
                "objekte:stored",
                "objekte:regex",
            ),
        ] {
            request.mode = mode;
            request.pattern = pattern.into();
            let legacy = repository.search_text(&request).unwrap();
            let registered = repository
                .search_text_with(&request, representation, operator)
                .unwrap();
            assert_eq!(legacy.total, registered.total);
            if mode == TextMode::Word {
                assert_eq!(
                    legacy.metrics.fts_candidates,
                    registered.metrics.fts_candidates
                );
                assert!(registered.metrics.fts_candidates < 3);
            }
            assert_eq!(
                legacy
                    .hits
                    .iter()
                    .map(|hit| (&hit.record.id, &hit.fields, &hit.matched_paths))
                    .collect::<Vec<_>>(),
                registered
                    .hits
                    .iter()
                    .map(|hit| (&hit.record.id, &hit.fields, &hit.matched_paths))
                    .collect::<Vec<_>>()
            );
        }
        for (field, pattern, representation) in [
            ("md", "Help", "objekte:visible-markdown"),
            ("md", "strasse", "objekte:visible-markdown"),
            ("xml", "Help", "objekte:visible-xml"),
            ("xml", "broken", "objekte:visible-xml"),
        ] {
            request.fields = Some(vec![field.into()]);
            request.mode = TextMode::Word;
            request.pattern = pattern.into();
            let legacy = repository.search_text(&request).unwrap();
            let registered = repository
                .search_text_with(&request, representation, "objekte:word")
                .unwrap();
            assert_eq!(legacy.total, registered.total);
            assert_eq!(
                legacy.metrics.fts_candidates,
                registered.metrics.fts_candidates
            );
            assert_eq!(
                legacy
                    .hits
                    .iter()
                    .map(|hit| (&hit.record.id, &hit.fields, &hit.matched_paths))
                    .collect::<Vec<_>>(),
                registered
                    .hits
                    .iter()
                    .map(|hit| (&hit.record.id, &hit.fields, &hit.matched_paths))
                    .collect::<Vec<_>>()
            );
        }

        repository.save_bigram_cache("books").unwrap();
        let mut restored = Repository::builder(root.path())
            .register_representation(RenderedRepresentation::new(TextFormat::Html).unwrap())
            .unwrap()
            .register_representation(TrimmedRepresentation::default())
            .unwrap()
            .register_operator(NativeTextOperator::new(TextMode::Word, false))
            .unwrap()
            .open()
            .unwrap();
        request.fields = Some(vec!["html".into()]);
        request.pattern = "Hello".into();
        let from_snapshot = restored
            .search_text_with(&request, "objekte:visible-html", "objekte:word")
            .unwrap();
        let native_snapshot = restored.search_text(&request).unwrap();
        assert_eq!(from_snapshot.total, native_snapshot.total);
        assert_eq!(
            from_snapshot.metrics.fts_candidates,
            native_snapshot.metrics.fts_candidates
        );
        assert_eq!(from_snapshot.hits[0].record.id, "a");

        request.pattern = "He".into();
        request.mode = TextMode::Word;
        request.fields = Some(vec!["html".into()]);
        let raw = repository
            .search_text_with(&request, "objekte:stored", "example:prefix")
            .unwrap();
        assert_eq!(raw.total, Some(1));
        assert_eq!(raw.hits[0].record.id, "a");
        let cached = repository
            .search_text_with(&request, "objekte:stored", "example:prefix")
            .unwrap();
        assert_eq!(cached.hits[0].record.id, "a");

        request.pattern = "Hel".into();
        request.fields = Some(vec!["body".into()]);
        let accelerated = repository
            .search_text_with(&request, "objekte:stored", "example:prefix")
            .unwrap();
        let scan_only = repository
            .search_text_with(&request, "example:trimmed", "example:prefix")
            .unwrap();
        assert_eq!(accelerated.total, Some(1));
        assert_eq!(accelerated.hits[0].record.id, "b");
        assert_eq!(scan_only.total, Some(2));
        assert!(accelerated.metrics.fts_candidates < scan_only.metrics.fts_candidates);

        let mut streamed = Vec::new();
        let summary = repository
            .search_text_stream_with(&request, "example:trimmed", "example:prefix", |_, hit| {
                streamed.push(hit.record.id.clone());
                Ok(())
            })
            .unwrap();
        assert_eq!(summary.total, Some(2));
        assert_eq!(streamed.len(), 2);
        let failed = repository.search_text_stream_with(
            &request,
            "example:trimmed",
            "example:prefix",
            |_, _| Err(Error::Request("cancelled by consumer".into())),
        );
        assert!(
            matches!(failed, Err(Error::Request(message)) if message == "cancelled by consumer")
        );

        let mut direct_options = Options::new();
        direct_options.cache = CacheMode::Disabled;
        let mut direct = Repository::builder(root.path())
            .options(direct_options)
            .register_representation(StoredRepresentation::default())
            .unwrap()
            .register_representation(TrimmedRepresentation::default())
            .unwrap()
            .register_operator(PrefixOperator::default())
            .unwrap()
            .open()
            .unwrap();
        assert_eq!(
            direct
                .search_text_with(&request, "objekte:stored", "example:prefix")
                .unwrap()
                .total,
            Some(1)
        );

        fs::write(
            books.join("c.json"),
            r#"{"body":"Hello C","html":"He<b>llo</b>"}"#,
        )
        .unwrap();
        repository.refresh("books").unwrap();
        let markup = TextQuery {
            pattern: "Hello".into(),
            collections: Some(vec!["books".into()]),
            fields: Some(vec!["html".into()]),
            count: true,
            ..Default::default()
        };
        let native_markup = repository.search_text(&markup).unwrap();
        let registered_markup = repository
            .search_text_with(&markup, "objekte:visible-html", "objekte:word")
            .unwrap();
        assert_eq!(native_markup.total, Some(2));
        assert_eq!(registered_markup.total, native_markup.total);
        assert_eq!(
            registered_markup.metrics.fts_candidates,
            native_markup.metrics.fts_candidates
        );
        assert_eq!(
            repository
                .search_text_with(&request, "example:trimmed", "example:prefix")
                .unwrap()
                .total,
            Some(3)
        );
        fs::write(
            books.join(".editor/schema.json"),
            r#"{"type":"object","properties":{"body":{"type":"string","x-trimmed":false},"html":{"type":"string","x-full-text":"html"},"md":{"type":"string","x-full-text":"markdown"},"xml":{"type":"string","x-full-text":"xml"}}}"#,
        )
        .unwrap();
        repository.reload().unwrap();
        assert_eq!(
            repository
                .search_text_with(&request, "example:trimmed", "example:prefix")
                .unwrap()
                .total,
            Some(0)
        );
    }
}
