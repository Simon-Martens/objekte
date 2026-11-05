//! Downstream compilation and behavior checks using only public core APIs.

use objekte_core::{
    Error, Result,
    extensions::{
        format::{DecodedRecord, ScalarValue},
        identity::{DataKind, Dependency, Descriptor, SemanticVersion},
        operator::{CandidateConstraint, CompiledOperator, QueryOperator},
        projection::{
            BindingContext, Extraction, RecordAccess, TextRepresentation, TypedValue,
            ValueExtractor,
        },
        query::{ExtensionBindings, LogicalPlan, LogicalPredicate, QueryFrontend},
    },
};
use serde_json::Value;
use std::{collections::BTreeMap, fs, io::Read, path::PathBuf, time::UNIX_EPOCH};


/// Select direct `.md` files; the source identity includes the real filename.
pub struct MarkdownProvider {
    descriptor: Descriptor,
    root: PathBuf,
    collection: String,
}


const MAX_MARKDOWN_SOURCE_BYTES: u64 = 64 * 1024 * 1024;


impl MarkdownProvider {
    pub fn new(root: PathBuf) -> Self {
        let collection = root
            .file_name()
            .and_then(|name| name.to_str())
            .expect("Markdown collection directory has a UTF-8 name")
            .to_owned();
        Self {
            descriptor: Descriptor::new(
                "example:markdown-files",
                SemanticVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                &root,
                DataKind::Source,
                DataKind::Source,
                vec![Dependency::FileName, Dependency::SourceBytes],
            ),
            root,
            collection,
        }
    }

    fn revision_metadata(metadata: &fs::Metadata, path: &std::path::Path) -> Result<String> {
        if !metadata.is_file() {
            return Err(Error::Request(
                "Markdown source must be a regular file".into(),
            ));
        }
        let modified = metadata.modified().map_err(|source| Error::Io {
            path: path.into(),
            source,
        })?;
        let nanos = modified
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos().to_string())
            .unwrap_or_else(|error| format!("-{}", error.duration().as_nanos()));
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            format!(
                "{}:{}:{}:{}",
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec()
            )
        };
        #[cfg(not(unix))]
        let identity = String::new();
        Ok(format!("{}:{nanos}:{identity}", metadata.len()))
    }

    fn revision(path: &std::path::Path) -> Result<String> {
        let metadata = fs::symlink_metadata(path).map_err(|source| Error::Io {
            path: path.into(),
            source,
        })?;
        Self::revision_metadata(&metadata, path)
    }
}


impl objekte_core::extensions::source::SourceProvider for MarkdownProvider {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn may_select(&self, collection: &str) -> bool {
        collection == self.collection
    }

    fn enumerate(
        &self,
        collection: &str,
    ) -> Result<Vec<objekte_core::extensions::SourceDescriptor>> {
        if collection != self.collection {
            return Ok(Vec::new());
        }
        let mut sources = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(|source| Error::Io {
            path: self.root.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| Error::Io {
                path: self.root.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
                continue;
            }
            let filename = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::Request("Markdown filename must be UTF-8".into()))?;
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| Error::Request("Markdown record ID must be UTF-8".into()))?;
            sources.push(objekte_core::extensions::SourceDescriptor {
                collection: collection.into(),
                provider: self.descriptor.identity.clone(),
                source_id: filename.clone(),
                record_id: id.into(),
                locator: PathBuf::from(filename),
                format: "example:markdown".into(),
                revision: Self::revision(&path)?,
                metadata: BTreeMap::new(),
            });
        }
        Ok(sources)
    }

    fn read(
        &self,
        source: &objekte_core::extensions::SourceDescriptor,
        output: &mut Vec<u8>,
    ) -> Result<()> {
        if source.provider != self.descriptor.identity
            || source.locator.components().count() != 1
            || source
                .locator
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("md")
        {
            return Err(Error::Request("invalid Markdown source descriptor".into()));
        }
        let path = self.root.join(&source.locator);
        #[cfg(unix)]
        let mut file = {
            use std::fs::File;
            let directory = File::from(
                rustix::fs::open(
                    &self.root,
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::DIRECTORY
                        | rustix::fs::OFlags::NOFOLLOW
                        | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(|error| Error::Io {
                    path: self.root.clone(),
                    source: error.into(),
                })?,
            );
            File::from(
                rustix::fs::openat(
                    &directory,
                    &source.locator,
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::NOFOLLOW
                        | rustix::fs::OFlags::CLOEXEC
                        | rustix::fs::OFlags::NONBLOCK,
                    rustix::fs::Mode::empty(),
                )
                .map_err(|error| Error::Io {
                    path: path.clone(),
                    source: error.into(),
                })?,
            )
        };
        #[cfg(not(unix))]
        let mut file = fs::File::open(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        let before_metadata = file.metadata().map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        let before = Self::revision_metadata(&before_metadata, &path)?;
        if before != source.revision {
            return Err(Error::Unstable(path));
        }
        if before_metadata.len() > MAX_MARKDOWN_SOURCE_BYTES {
            return Err(Error::Request("Markdown source exceeds 64 MiB".into()));
        }
        output.clear();
        (&mut file)
            .take(MAX_MARKDOWN_SOURCE_BYTES + 1)
            .read_to_end(output)
            .map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
        let after_metadata = file.metadata().map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        let after = Self::revision_metadata(&after_metadata, &path)?;
        if after != source.revision
            || output.len() as u64 != after_metadata.len()
            || output.len() as u64 > MAX_MARKDOWN_SOURCE_BYTES
        {
            return Err(Error::Unstable(path));
        }
        Ok(())
    }
}


/// One UTF-8 Markdown file becomes one logical record with a `body` leaf.
pub struct MarkdownDecoder(Descriptor);


struct MarkdownRecord<'a> {
    body: &'a str,
}


impl RecordAccess for MarkdownRecord<'_> {
    fn path_present(&self, path: &objekte_core::JsonPath) -> Result<bool> {
        Ok(matches!(path.as_slice(), [objekte_core::JsonPathStep::Key(name)] if name == "body"))
    }

    fn visit_path(
        &self,
        path: &objekte_core::JsonPath,
        visit: &mut dyn FnMut(ScalarValue<'_>) -> Result<()>,
    ) -> Result<usize> {
        if self.path_present(path)? {
            visit(ScalarValue::Text(self.body))?;
            Ok(1)
        } else {
            Ok(0)
        }
    }
}


impl DecodedRecord for MarkdownRecord<'_> {
    fn visit_leaves(
        &self,
        visit: &mut dyn FnMut(&str, ScalarValue<'_>) -> Result<()>,
    ) -> Result<()> {
        visit("/body", ScalarValue::Text(self.body))
    }
}


impl Default for MarkdownDecoder {
    fn default() -> Self {
        Self(Descriptor::new(
            "example:markdown",
            SemanticVersion {
                major: 1,
                minor: 0,
                patch: 0,
            },
            &(),
            DataKind::Source,
            DataKind::Record,
            vec![Dependency::SourceBytes],
        ))
    }
}


impl objekte_core::extensions::format::FormatDecoder for MarkdownDecoder {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }

    fn accepts(&self, source: &objekte_core::extensions::SourceDescriptor) -> bool {
        source.format == self.0.identity
    }

    fn decode(
        &self,
        bytes: &[u8],
        visit: &mut dyn FnMut(&dyn DecodedRecord) -> Result<()>,
    ) -> Result<()> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| Error::Request("Markdown source must be UTF-8".into()))?;
        visit(&MarkdownRecord { body: text })
    }
}


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

    fn can_skip_non_candidates(&self) -> bool {
        true
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


/// Extract exactly `YYYY` or `circa YYYY` from `publication_date`.
/// Ranges and other ambiguous spellings are errors, not partial matches.
pub struct PublicationYearExtractor {
    descriptor: Descriptor,
    source_path: objekte_core::JsonPath,
}


impl Default for PublicationYearExtractor {
    fn default() -> Self {
        Self {
            descriptor: Descriptor::new(
                "example:publication_year",
                SemanticVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                &("publication_date", "YYYY | circa YYYY"),
                DataKind::Record,
                DataKind::TypedValues,
                vec![Dependency::SourceBytes],
            ),
            source_path: objekte_core::JsonFilter::parse_path("publication_date")
                .expect("constant source path is valid"),
        }
    }
}


impl ValueExtractor for PublicationYearExtractor {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn binds(&self, context: &BindingContext<'_>) -> bool {
        context.path == "@derived:example:publication_year" && context.format == "derived"
    }

    fn extract(
        &self,
        record: &dyn RecordAccess,
        output: &mut Vec<TypedValue>,
    ) -> Result<Extraction> {
        if !record.path_present(&self.source_path)? {
            return Ok(Extraction::Missing);
        }
        record.visit_path(&self.source_path, &mut |value| {
            match value {
                ScalarValue::Null => output.push(TypedValue::Null),
                ScalarValue::Text(text) => {
                    let year = text.strip_prefix("circa ").unwrap_or(text);
                    if year.len() != 4 || !year.bytes().all(|byte| byte.is_ascii_digit()) {
                        return Err(Error::Request(format!(
                            "publication_date {text:?} is not YYYY or circa YYYY"
                        )));
                    }
                    output.push(TypedValue::Number(year.into()));
                }
                _ => {
                    return Err(Error::Request(
                        "publication_date must be text or null".into(),
                    ));
                }
            }
            Ok(())
        })?;
        Ok(Extraction::Present)
    }
}


/// Small structured request compiled entirely through public handles. The
/// engine owns the source scan, Boolean evaluation, sort and pagination.
pub struct ArchiveFrontend(Descriptor);


impl Default for ArchiveFrontend {
    fn default() -> Self {
        Self(Descriptor::new(
            "example:archive-search",
            SemanticVersion {
                major: 1,
                minor: 0,
                patch: 0,
            },
            &("archive-search", 1),
            DataKind::Query,
            DataKind::Query,
            vec![],
        ))
    }
}


impl QueryFrontend for ArchiveFrontend {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }

    fn compile(&self, request: &Value, bindings: &ExtensionBindings<'_>) -> Result<LogicalPlan> {
        let collection = request
            .get("collection")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Request("archive search requires collection".into()))?;
        let text_operator = bindings
            .operator("objekte:word")
            .ok_or_else(|| Error::Request("word operator is not registered".into()))?;
        let rendered = bindings
            .representation("objekte:visible-markdown")
            .ok_or_else(|| Error::Request("Markdown representation is not registered".into()))?;
        let prefix_operator = bindings
            .operator("example:prefix")
            .ok_or_else(|| Error::Request("prefix operator is not registered".into()))?;
        let stored = bindings
            .representation("objekte:stored")
            .ok_or_else(|| Error::Request("stored representation is not registered".into()))?;
        let year = bindings
            .extractor("example:publication_year")
            .ok_or_else(|| Error::Request("publication year is not registered".into()))?;
        let text_path = request
            .get("term_path")
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| Error::Request("term_path must be text".into()))
            })
            .transpose()?
            .unwrap_or("body");
        let mut predicates = Vec::new();
        if let Some(term) = request.get("term") {
            let term = term
                .as_str()
                .ok_or_else(|| Error::Request("archive term must be text".into()))?;
            predicates.push(LogicalPredicate::Call {
                operator: text_operator,
                representation: rendered,
                path: text_path.into(),
                parameters: Value::String(term.into()),
            });
        }
        for (name, predicate) in [
            (
                "min_year",
                objekte_core::DerivedPredicate::Ge as fn(TypedValue) -> _,
            ),
            (
                "max_year",
                objekte_core::DerivedPredicate::Le as fn(TypedValue) -> _,
            ),
        ] {
            if let Some(value) = request.get(name) {
                let year_value = value
                    .as_i64()
                    .ok_or_else(|| Error::Request(format!("{name} must be an integer")))?;
                predicates.push(LogicalPredicate::Derived {
                    extractor: year,
                    predicate: predicate(TypedValue::Number(year_value.to_string())),
                });
            }
        }
        if let Some(prefixes) = request.get("any_prefix") {
            let prefixes = prefixes
                .as_array()
                .ok_or_else(|| Error::Request("any_prefix must be an array".into()))?;
            predicates.push(LogicalPredicate::Or(
                prefixes
                    .iter()
                    .map(|prefix| {
                        let prefix = prefix.as_str().ok_or_else(|| {
                            Error::Request("any_prefix values must be text".into())
                        })?;
                        Ok(LogicalPredicate::Call {
                            operator: prefix_operator,
                            representation: stored,
                            path: "body".into(),
                            parameters: Value::String(prefix.into()),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            ));
        }
        if let Some(prefix) = request.get("exclude_prefix") {
            let prefix = prefix
                .as_str()
                .ok_or_else(|| Error::Request("exclude_prefix must be text".into()))?;
            predicates.push(LogicalPredicate::Not(Box::new(LogicalPredicate::Call {
                operator: prefix_operator,
                representation: stored,
                path: "body".into(),
                parameters: Value::String(prefix.into()),
            })));
        }
        let sort = request
            .get("sort_year")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            .then(|| objekte_core::Sort {
                field: "@derived:example:publication_year".into(),
                direction: objekte_core::Direction::Asc,
            })
            .into_iter()
            .collect();
        let offset = usize::try_from(request.get("offset").and_then(Value::as_u64).unwrap_or(0))
            .map_err(|_| Error::Request("offset exceeds platform size".into()))?;
        let limit = usize::try_from(request.get("limit").and_then(Value::as_u64).unwrap_or(50))
            .map_err(|_| Error::Request("limit exceeds platform size".into()))?;
        Ok(LogicalPlan {
            collections: vec![collection.into()],
            predicate: LogicalPredicate::And(predicates),
            sort,
            columns: vec!["body".into(), "publication_date".into()],
            offset,
            limit,
            count: request
                .get("count")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use objekte_core::{
        CacheMode, DerivedBackend, DerivedPredicate, DerivedQuery, DerivedSort, Direction,
        Freshness, Options, Repository, SortReduce, TextFormat, TextMode, TextQuery,
        extensions::{NativeTextOperator, RenderedRepresentation, StoredRepresentation},
    };
    use std::fs;


    struct PageFrontend(Descriptor);


    impl Default for PageFrontend {
        fn default() -> Self {
            Self(Descriptor::new(
                "example:pages",
                SemanticVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                &(),
                DataKind::Query,
                DataKind::Query,
                vec![],
            ))
        }
    }


    impl QueryFrontend for PageFrontend {
        fn descriptor(&self) -> &Descriptor {
            &self.0
        }

        fn compile(
            &self,
            request: &Value,
            bindings: &ExtensionBindings<'_>,
        ) -> Result<LogicalPlan> {
            let predicate = if request.get("prefix").is_some() {
                LogicalPredicate::Call {
                    operator: bindings.operator("example:prefix").unwrap(),
                    representation: bindings.representation("objekte:stored").unwrap(),
                    path: "body".into(),
                    parameters: request["prefix"].clone(),
                }
            } else {
                LogicalPredicate::And(Vec::new())
            };
            Ok(LogicalPlan {
                collections: vec!["alpha".into(), "beta".into()],
                predicate,
                sort: vec![objekte_core::Sort {
                    field: if request.get("year") == Some(&Value::Bool(true)) {
                        "@derived:example:publication_year"
                    } else {
                        "@id"
                    }
                    .into(),
                    direction: Direction::Asc,
                }],
                columns: vec!["body".into()],
                offset: request.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
                limit: request.get("limit").and_then(Value::as_u64).unwrap_or(1) as usize,
                count: true,
            })
        }
    }


    fn page_fixture() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        for collection in ["alpha", "beta"] {
            let directory = temp.path().join("data").join(collection);
            fs::create_dir_all(&directory).unwrap();
            for id in ["a", "b", "c"] {
                fs::write(
                    directory.join(format!("{id}.json")),
                    format!(r#"{{"body":"target {collection} {id}","publication_date":"1848"}}"#),
                )
                .unwrap();
            }
        }
        temp
    }

    fn page_repository(
        root: &std::path::Path,
        budget: usize,
        operator: PrefixOperator,
        extractor: PublicationYearExtractor,
        reverse_registration: bool,
    ) -> Repository {
        let builder = Repository::builder(root)
            .options(Options {
                query_cache_bytes: budget,
                ..Default::default()
            })
            .register_representation(StoredRepresentation::default())
            .unwrap();
        let builder = if reverse_registration {
            builder
                .register_extractor(extractor)
                .unwrap()
                .register_operator(operator)
                .unwrap()
        } else {
            builder
                .register_operator(operator)
                .unwrap()
                .register_extractor(extractor)
                .unwrap()
        };
        builder
            .register_frontend(PageFrontend::default())
            .unwrap()
            .open()
            .unwrap()
    }

    #[test]
    fn completed_frontend_pages_use_only_requested_slots_and_current_inventory() {
        let temp = page_fixture();
        let mut repository = page_repository(
            temp.path(),
            64 * 1024 * 1024,
            PrefixOperator::default(),
            PublicationYearExtractor::default(),
            false,
        );
        let expected = [
            "alpha/a", "beta/a", "alpha/b", "beta/b", "alpha/c", "beta/c",
        ];
        let request = serde_json::json!({"offset":0,"limit":1});
        let first = repository
            .query_frontend("example:pages", &request)
            .unwrap();
        assert_eq!(first.total, Some(expected.len()));
        assert_eq!(first.query_plan, ["extension-exact-scan"]);
        for (offset, limit) in [(0, 1), (2, 2), (5, 1), (6, 1), (i64::MAX as usize, 1)] {
            let page = repository
                .query_frontend(
                    "example:pages",
                    &serde_json::json!({"offset":offset,"limit":limit}),
                )
                .unwrap();
            let ids = page
                .rows
                .iter()
                .map(|row| format!("{}/{}", row.source.collection, row.source.record_id))
                .collect::<Vec<_>>();
            let end = expected.len().min(offset.saturating_add(limit));
            assert_eq!(ids, expected.get(offset..end).unwrap_or(&[]));
            assert_eq!(page.total, Some(expected.len()));
            assert_eq!(page.query_plan, ["extension-ordered-cache"]);
            assert_eq!(page.metrics.enumerations, 0);
            assert_eq!(page.metrics.source_reads, page.rows.len() as u64);
            assert_eq!(page.metrics.parses, page.rows.len() as u64);
        }
        let alpha = temp.path().join("data/alpha");
        fs::write(
            alpha.join("a.json"),
            r#"{"body":"edited","publication_date":"1848"}"#,
        )
        .unwrap();
        repository.refresh("alpha").unwrap();
        let edited = repository
            .query_frontend("example:pages", &request)
            .unwrap();
        assert_eq!(edited.query_plan, ["extension-exact-scan"]);
        assert_eq!(
            edited.rows[0].values["body"],
            [TypedValue::Text("edited".into())]
        );
        fs::write(
            temp.path().join("data/beta/d.json"),
            r#"{"body":"inserted","publication_date":"1848"}"#,
        )
        .unwrap();
        repository.refresh("beta").unwrap();
        assert_eq!(
            repository
                .query_frontend("example:pages", &request)
                .unwrap()
                .total,
            Some(7)
        );
        fs::remove_file(alpha.join("b.json")).unwrap();
        repository.refresh("alpha").unwrap();
        let deleted = repository
            .query_frontend("example:pages", &request)
            .unwrap();
        assert_eq!(deleted.total, Some(6));
        assert_eq!(deleted.query_plan, ["extension-exact-scan"]);
        for budget in [0, 1] {
            let mut uncached = page_repository(
                temp.path(),
                budget,
                PrefixOperator::default(),
                PublicationYearExtractor::default(),
                false,
            );
            for _ in 0..2 {
                assert_eq!(
                    uncached
                        .query_frontend("example:pages", &request)
                        .unwrap()
                        .query_plan,
                    ["extension-exact-scan"]
                );
            }
        }
    }

    #[test]
    fn frontend_cache_depends_only_on_bound_definitions() {
        let temp = page_fixture();
        let stable = serde_json::json!({"offset":0,"limit":1});
        let mut reference = page_repository(
            temp.path(),
            64 * 1024 * 1024,
            PrefixOperator::default(),
            PublicationYearExtractor::default(),
            false,
        );
        let first = reference.query_frontend("example:pages", &stable).unwrap();
        let mut reordered = page_repository(
            temp.path(),
            64 * 1024 * 1024,
            PrefixOperator::default(),
            PublicationYearExtractor::default(),
            true,
        );
        assert_eq!(
            reordered
                .query_frontend("example:pages", &stable)
                .unwrap()
                .identity,
            first.identity
        );
        assert_eq!(
            reference
                .query_frontend("example:pages", &serde_json::json!({"offset":4,"limit":1}))
                .unwrap()
                .query_plan,
            ["extension-ordered-cache"]
        );
        assert_eq!(
            reference
                .query_frontend(
                    "example:pages",
                    &serde_json::json!({"prefix":"target","limit":1})
                )
                .unwrap()
                .query_plan,
            ["extension-exact-scan"]
        );
        assert_eq!(
            reference
                .query_frontend("example:pages", &serde_json::json!({"year":true,"limit":1}))
                .unwrap()
                .query_plan,
            ["extension-exact-scan"]
        );

        let mut unused_operator = PrefixOperator::default();
        unused_operator.descriptor.version.patch += 1;
        unused_operator
            .descriptor
            .dependencies
            .push(Dependency::Volatile);
        let mut unused_extractor = PublicationYearExtractor::default();
        unused_extractor.descriptor.version.patch += 1;
        unused_extractor
            .descriptor
            .dependencies
            .push(Dependency::Volatile);
        let mut volatile = page_repository(
            temp.path(),
            64 * 1024 * 1024,
            unused_operator,
            unused_extractor,
            true,
        );
        let unchanged = volatile.query_frontend("example:pages", &stable).unwrap();
        assert_eq!(unchanged.identity, first.identity);
        assert_eq!(
            volatile
                .query_frontend("example:pages", &serde_json::json!({"offset":3,"limit":1}))
                .unwrap()
                .query_plan,
            ["extension-ordered-cache"]
        );
        for request in [
            serde_json::json!({"prefix":"target","limit":1}),
            serde_json::json!({"year":true,"limit":1}),
        ] {
            let first_use = volatile.query_frontend("example:pages", &request).unwrap();
            let second_use = volatile.query_frontend("example:pages", &request).unwrap();
            assert_ne!(first_use.query_plan, ["extension-ordered-cache"]);
            assert_ne!(second_use.query_plan, ["extension-ordered-cache"]);
        }
        let stable_used = reference
            .query_frontend(
                "example:pages",
                &serde_json::json!({"prefix":"target","limit":1}),
            )
            .unwrap();
        let changed_used = volatile
            .query_frontend(
                "example:pages",
                &serde_json::json!({"prefix":"target","limit":1}),
            )
            .unwrap();
        assert_ne!(stable_used.identity, changed_used.identity);
        let stable_sort = reference
            .query_frontend("example:pages", &serde_json::json!({"year":true,"limit":1}))
            .unwrap();
        let changed_sort = volatile
            .query_frontend("example:pages", &serde_json::json!({"year":true,"limit":1}))
            .unwrap();
        assert_ne!(stable_sort.identity, changed_sort.identity);
    }

    #[test]
    fn empty_applicable_provider_and_decoder_are_selection_dependencies() {
        let temp = page_fixture();
        let beta = temp.path().join("data/beta");
        let request = serde_json::json!({"offset":0,"limit":1});
        let mut plain = page_repository(
            temp.path(),
            64 * 1024 * 1024,
            PrefixOperator::default(),
            PublicationYearExtractor::default(),
            false,
        );
        let plain_identity = plain
            .query_frontend("example:pages", &request)
            .unwrap()
            .identity;
        let open = |decoder_patch| {
            let mut decoder = MarkdownDecoder::default();
            decoder.0.version.patch = decoder_patch;
            Repository::builder(temp.path())
                .register_source(MarkdownProvider::new(beta.clone()))
                .unwrap()
                .register_format(decoder)
                .unwrap()
                .register_frontend(PageFrontend::default())
                .unwrap()
                .open()
                .unwrap()
        };
        let mut selected = open(0);
        let initial = selected.query_frontend("example:pages", &request).unwrap();
        assert_ne!(initial.identity, plain_identity);
        assert_eq!(initial.total, Some(6));
        assert_eq!(
            selected
                .query_frontend("example:pages", &request)
                .unwrap()
                .query_plan,
            ["extension-ordered-cache"]
        );
        let mut changed_decoder = open(1);
        assert_ne!(
            changed_decoder
                .query_frontend("example:pages", &request)
                .unwrap()
                .identity,
            initial.identity
        );
        fs::write(beta.join("d.md"), "Markdown body").unwrap();
        selected.refresh("beta").unwrap();
        let refreshed = selected.query_frontend("example:pages", &request).unwrap();
        assert_eq!(refreshed.total, Some(7));
        assert_eq!(refreshed.query_plan, ["extension-exact-scan"]);
    }

    #[test]
    fn archive_frontend_composes_markup_year_boolean_sort_and_page() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/archive");
        fs::create_dir_all(directory.join(".editor")).unwrap();
        fs::write(
            directory.join(".editor/schema.json"),
            r#"{"type":"object","properties":{"body":{"type":"string","x-full-text":"markdown"},"publication_date":{"type":"string"}}}"#,
        )
        .unwrap();
        for (id, body, date) in [
            ("a", "History of A", "circa 1848"),
            ("b", "HISTORY of B", "1850"),
            ("c", "History of C", "1850"),
            ("d", "History of D", "1852"),
        ] {
            fs::write(
                directory.join(format!("{id}.json")),
                serde_json::to_vec(&serde_json::json!({
                    "body": body,
                    "publication_date": date,
                }))
                .unwrap(),
            )
            .unwrap();
        }
        fs::write(directory.join("m.md"), "# History of Markdown").unwrap();
        let mut repository = Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(directory.clone()))
            .unwrap()
            .register_format(MarkdownDecoder::default())
            .unwrap()
            .register_representation(StoredRepresentation::default())
            .unwrap()
            .register_representation(RenderedRepresentation::new(TextFormat::Markdown).unwrap())
            .unwrap()
            .register_operator(PrefixOperator::default())
            .unwrap()
            .register_operator(NativeTextOperator::new(TextMode::Word, false))
            .unwrap()
            .register_extractor(PublicationYearExtractor::default())
            .unwrap()
            .register_frontend(ArchiveFrontend::default())
            .unwrap()
            .open()
            .unwrap();
        let request = serde_json::json!({
            "collection":"archive", "term":"history", "min_year":1848,
            "max_year":1850, "sort_year":true, "offset":1, "limit":2,
            "count":true,
        });
        let page = repository
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(page.total, Some(3));
        assert!(!page.truncated);
        assert_eq!(
            page.rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>(),
            ["b", "c"]
        );
        assert_eq!(
            page.rows[0].values["publication_date"],
            vec![TypedValue::Text("1850".into())]
        );
        let cached_page = repository
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(cached_page.identity, page.identity);
        assert_eq!(cached_page.query_plan, ["extension-ordered-cache"]);
        assert_eq!(cached_page.metrics.ordered_result_hits, 1);
        assert_eq!(cached_page.metrics.enumerations, 0);
        assert_eq!(cached_page.metrics.source_reads, 2);
        let boolean = serde_json::json!({
            "collection":"archive", "term":"history", "min_year":1848,
            "max_year":1850, "any_prefix":["History","HISTORY"],
            "exclude_prefix":"History of C", "count":true,
        });
        let result = repository
            .query_frontend("example:archive-search", &boolean)
            .unwrap();
        assert_eq!(result.total, Some(2));
        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        let markup = serde_json::json!({
            "collection":"archive", "term":"History of Markdown", "count":true,
        });
        let result = repository
            .query_frontend("example:archive-search", &markup)
            .unwrap();
        assert_eq!(result.total, Some(1));
        assert_eq!(result.rows[0].source.locator, PathBuf::from("m.md"));
        assert_eq!(
            result.rows[0].values["publication_date"],
            Vec::<TypedValue>::new()
        );
        fs::write(directory.join("n.md"), "# History of Markdown two").unwrap();
        // MetadataOnOpen keeps the selected source snapshot until explicit refresh.
        assert_eq!(
            repository
                .query_frontend("example:archive-search", &markup)
                .unwrap()
                .total,
            Some(1)
        );
        repository.refresh("archive").unwrap();
        assert_eq!(
            repository
                .query_frontend("example:archive-search", &markup)
                .unwrap()
                .total,
            Some(2)
        );
        fs::rename(directory.join("n.md"), directory.join("renamed.md")).unwrap();
        repository.refresh("archive").unwrap();
        assert_eq!(
            repository
                .query_frontend("example:archive-search", &markup)
                .unwrap()
                .total,
            Some(2)
        );
        fs::remove_file(directory.join("renamed.md")).unwrap();
        repository.refresh("archive").unwrap();
        assert_eq!(
            repository
                .query_frontend("example:archive-search", &markup)
                .unwrap()
                .total,
            Some(1)
        );
        repository.reload().unwrap();
        assert_eq!(
            repository
                .query_frontend("example:archive-search", &markup)
                .unwrap()
                .total,
            Some(1)
        );
        fs::write(directory.join("m.md"), [0xff]).unwrap();
        assert!(
            repository
                .query_frontend("example:archive-search", &markup)
                .is_err()
        );
    }

    #[test]
    fn provider_refresh_agrees_with_fresh_exact_repository_after_mutations() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/archive");
        fs::create_dir_all(directory.join(".editor")).unwrap();
        fs::write(
            directory.join(".editor/schema.json"),
            r#"{"type":"object","properties":{"body":{"type":"string","x-full-text":"markdown"}}}"#,
        )
        .unwrap();
        fs::write(directory.join("a.json"), r#"{"body":"History A"}"#).unwrap();
        fs::write(directory.join("b.md"), "# History B").unwrap();
        let open = || {
            Repository::builder(temp.path())
                .register_source(MarkdownProvider::new(directory.clone()))
                .unwrap()
                .register_format(MarkdownDecoder::default())
                .unwrap()
                .register_representation(StoredRepresentation::default())
                .unwrap()
                .register_representation(RenderedRepresentation::new(TextFormat::Markdown).unwrap())
                .unwrap()
                .register_operator(PrefixOperator::default())
                .unwrap()
                .register_operator(NativeTextOperator::new(TextMode::Word, false))
                .unwrap()
                .register_extractor(PublicationYearExtractor::default())
                .unwrap()
                .register_frontend(ArchiveFrontend::default())
                .unwrap()
                .open()
                .unwrap()
        };
        let signature = |repository: &mut Repository, request: &Value| {
            repository
                .query_frontend("example:archive-search", request)
                .map(|result| {
                    (
                        result.total,
                        result.truncated,
                        result
                            .rows
                            .into_iter()
                            .map(|row| (row.source.record_id, row.source.locator, row.values))
                            .collect::<Vec<_>>(),
                    )
                })
                .map_err(|error| error.to_string())
        };
        let mut incremental = open();
        for mutation in 0..6 {
            match mutation {
                1 => fs::write(directory.join("a.json"), r#"{"body":"Other A"}"#).unwrap(),
                2 => fs::write(directory.join("c.md"), "# History C").unwrap(),
                3 => fs::rename(directory.join("b.md"), directory.join("d.md")).unwrap(),
                4 => fs::remove_file(directory.join("c.md")).unwrap(),
                5 => fs::write(directory.join("d.md"), [0xff]).unwrap(),
                _ => {}
            }
            if mutation != 0 {
                incremental.refresh("archive").unwrap();
            }
            let mut fresh = open();
            for limit in [1, 10] {
                let request = serde_json::json!({
                    "collection":"archive", "term":"history", "limit":limit, "count":true,
                });
                assert_eq!(
                    signature(&mut incremental, &request),
                    signature(&mut fresh, &request),
                    "step {mutation}, limit {limit}"
                );
            }
        }
    }

    #[test]
    fn representation_postings_narrow_mixed_sources_without_changing_results() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/archive");
        fs::create_dir_all(directory.join(".editor")).unwrap();
        fs::write(
            directory.join(".editor/schema.json"),
            r#"{"type":"object","properties":{"body":{"type":"string","x-full-text":"markdown"}}}"#,
        )
        .unwrap();
        fs::write(directory.join("a.json"), r#"{"body":"rareterm Alpha"}"#).unwrap();
        fs::write(directory.join("b.md"), "# common Beta").unwrap();
        fs::write(directory.join("c.md"), "# different Gamma").unwrap();
        fs::write(directory.join("d.json"), r#"{"body":"rareterm Delta"}"#).unwrap();
        let open = |freshness: Freshness| {
            Repository::builder(temp.path())
                .options(Options {
                    query_cache_bytes: 0,
                    freshness,
                    ..Default::default()
                })
                .register_source(MarkdownProvider::new(directory.clone()))
                .unwrap()
                .register_format(MarkdownDecoder::default())
                .unwrap()
                .register_representation(StoredRepresentation::default())
                .unwrap()
                .register_representation(RenderedRepresentation::new(TextFormat::Markdown).unwrap())
                .unwrap()
                .register_operator(PrefixOperator::default())
                .unwrap()
                .register_operator(NativeTextOperator::new(TextMode::Word, false))
                .unwrap()
                .register_extractor(PublicationYearExtractor::default())
                .unwrap()
                .register_frontend(ArchiveFrontend::default())
                .unwrap()
                .open()
                .unwrap()
        };
        let mut scan = open(Freshness::MetadataOnOpen);
        let mut indexed = open(Freshness::MetadataOnOpen);
        assert_eq!(
            indexed
                .build_representation_index("archive", "objekte:visible-markdown", "body")
                .unwrap(),
            4
        );
        indexed
            .build_representation_index("archive", "objekte:stored", "body")
            .unwrap();
        let snapshot = temp.path().join("selected-text.snapshot");
        indexed
            .save_selected_representation_snapshot(
                "archive",
                "objekte:visible-markdown",
                "body",
                &snapshot,
            )
            .unwrap();
        let mut restored = open(Freshness::MetadataOnOpen);
        assert!(
            restored
                .restore_selected_representation_snapshot(
                    "archive",
                    "objekte:visible-markdown",
                    "body",
                    &snapshot,
                )
                .unwrap()
        );
        assert_eq!(restored.metrics().source_reads, 0);
        assert_eq!(restored.metrics().parses, 0);
        let saved_request = serde_json::json!({
            "collection":"archive", "term":"rareterm", "count":true,
        });
        let saved_scan = scan
            .query_frontend("example:archive-search", &saved_request)
            .unwrap();
        let saved_result = restored
            .query_frontend("example:archive-search", &saved_request)
            .unwrap();
        assert_eq!(saved_result.query_plan, ["extension-index-candidates"]);
        assert_eq!(saved_result.total, saved_scan.total);
        assert_eq!(
            saved_result
                .rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>(),
            saved_scan
                .rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>()
        );
        let mut corrupted_bytes = fs::read(&snapshot).unwrap();
        let last = corrupted_bytes.len() - 1;
        corrupted_bytes[last] ^= 1;
        fs::write(&snapshot, corrupted_bytes).unwrap();
        let mut corrupted = open(Freshness::MetadataOnOpen);
        assert!(
            !corrupted
                .restore_selected_representation_snapshot(
                    "archive",
                    "objekte:visible-markdown",
                    "body",
                    &snapshot,
                )
                .unwrap()
        );
        indexed
            .save_selected_representation_snapshot(
                "archive",
                "objekte:visible-markdown",
                "body",
                &snapshot,
            )
            .unwrap();
        let mut changed_decoder = MarkdownDecoder::default();
        changed_decoder.0.version.patch += 1;
        let mut changed_semantics = Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(directory.clone()))
            .unwrap()
            .register_format(changed_decoder)
            .unwrap()
            .register_representation(StoredRepresentation::default())
            .unwrap()
            .register_representation(RenderedRepresentation::new(TextFormat::Markdown).unwrap())
            .unwrap()
            .register_operator(PrefixOperator::default())
            .unwrap()
            .register_operator(NativeTextOperator::new(TextMode::Word, false))
            .unwrap()
            .register_extractor(PublicationYearExtractor::default())
            .unwrap()
            .register_frontend(ArchiveFrontend::default())
            .unwrap()
            .open()
            .unwrap();
        assert!(
            !changed_semantics
                .restore_selected_representation_snapshot(
                    "archive",
                    "objekte:visible-markdown",
                    "body",
                    &snapshot,
                )
                .unwrap()
        );
        let mut companion_name = snapshot.as_os_str().to_os_string();
        companion_name.push(".selected-meta");
        let companion = std::path::PathBuf::from(companion_name);
        let mut metadata: Value = serde_json::from_slice(&fs::read(&companion).unwrap()).unwrap();
        metadata["version"] = serde_json::json!(1);
        fs::write(&companion, serde_json::to_vec(&metadata).unwrap()).unwrap();
        let mut incompatible = open(Freshness::MetadataOnOpen);
        assert!(
            !incompatible
                .restore_selected_representation_snapshot(
                    "archive",
                    "objekte:visible-markdown",
                    "body",
                    &snapshot,
                )
                .unwrap()
        );
        indexed
            .save_selected_representation_snapshot(
                "archive",
                "objekte:visible-markdown",
                "body",
                &snapshot,
            )
            .unwrap();
        for (request, uses_index) in [
            (
                serde_json::json!({"collection":"archive", "term":"rareterm", "offset":1, "limit":1, "count":true}),
                true,
            ),
            (
                serde_json::json!({"collection":"archive", "any_prefix":["rareterm", "# common"], "count":true}),
                true,
            ),
            (
                serde_json::json!({"collection":"archive", "exclude_prefix":"# common", "count":true}),
                false,
            ),
            (
                serde_json::json!({"collection":"archive", "term":"absent", "any_prefix":["# common"], "count":true}),
                true,
            ),
        ] {
            let expected = scan
                .query_frontend("example:archive-search", &request)
                .unwrap();
            let actual = indexed
                .query_frontend("example:archive-search", &request)
                .unwrap();
            assert_eq!(actual.identity, expected.identity);
            assert_eq!(actual.total, expected.total);
            assert_eq!(actual.truncated, expected.truncated);
            assert_eq!(
                actual
                    .rows
                    .iter()
                    .map(|row| (&row.source.record_id, &row.values))
                    .collect::<Vec<_>>(),
                expected
                    .rows
                    .iter()
                    .map(|row| (&row.source.record_id, &row.values))
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                actual.query_plan,
                [if uses_index {
                    "extension-index-candidates"
                } else {
                    "extension-exact-scan"
                }]
            );
            if uses_index {
                assert!(actual.metrics.source_reads < expected.metrics.source_reads);
            } else {
                assert_eq!(actual.metrics.source_reads, expected.metrics.source_reads);
            }
        }
        fs::write(directory.join("c.md"), "# rareterm Gamma").unwrap();
        let mut stale = open(Freshness::MetadataOnOpen);
        assert!(
            !stale
                .restore_selected_representation_snapshot(
                    "archive",
                    "objekte:visible-markdown",
                    "body",
                    &snapshot,
                )
                .unwrap()
        );
        indexed.refresh("archive").unwrap();
        let request = serde_json::json!({
            "collection":"archive", "term":"rareterm", "count":true,
        });
        let mut fresh = open(Freshness::MetadataOnOpen);
        let expected = fresh
            .query_frontend("example:archive-search", &request)
            .unwrap();
        let without_index = indexed
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(without_index.query_plan, ["extension-exact-scan"]);
        assert_eq!(without_index.total, expected.total);
        indexed
            .build_representation_index("archive", "objekte:visible-markdown", "body")
            .unwrap();
        let rebuilt = indexed
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(rebuilt.query_plan, ["extension-index-candidates"]);
        assert_eq!(rebuilt.total, expected.total);
        assert_eq!(
            rebuilt
                .rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>(),
            expected
                .rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>()
        );
        let mut on_request = open(Freshness::MetadataOnRequest);
        on_request
            .build_representation_index("archive", "objekte:visible-markdown", "body")
            .unwrap();
        fs::write(directory.join("b.md"), "# rareterm Beta").unwrap();
        let after_unannounced_edit = on_request
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(after_unannounced_edit.total, Some(4));
        assert_eq!(after_unannounced_edit.query_plan, ["extension-exact-scan"]);
        assert!(
            open(Freshness::HashOnRequest)
                .build_representation_index("archive", "objekte:visible-markdown", "body")
                .is_err()
        );
        let mut disabled = Repository::builder(temp.path())
            .options(Options {
                cache: CacheMode::Disabled,
                ..Default::default()
            })
            .register_representation(StoredRepresentation::default())
            .unwrap()
            .open()
            .unwrap();
        assert!(
            disabled
                .build_representation_index("archive", "objekte:stored", "body")
                .is_err()
        );
        fs::write(directory.join("c.md"), [0xff]).unwrap();
        indexed.refresh("archive").unwrap();
        assert!(
            indexed
                .build_representation_index("archive", "objekte:visible-markdown", "body")
                .is_err()
        );
        assert!(
            indexed
                .query_frontend("example:archive-search", &request)
                .is_err()
        );
    }

    #[test]
    fn selected_derived_posting_matches_mixed_source_scan_and_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/archive");
        fs::create_dir_all(&directory).unwrap();
        for (id, date) in [
            ("a", serde_json::json!("1848")),
            ("b", serde_json::json!("circa 1852")),
            ("c", serde_json::Value::Null),
            ("d", serde_json::json!(["1848", "1860"])),
        ] {
            fs::write(
                directory.join(format!("{id}.json")),
                serde_json::to_vec(&serde_json::json!({
                    "body": format!("Entry {id}"), "publication_date": date,
                }))
                .unwrap(),
            )
            .unwrap();
        }
        fs::write(directory.join("e.md"), "# No date").unwrap();
        let open = |freshness: Freshness| {
            Repository::builder(temp.path())
                .options(Options {
                    query_cache_bytes: 0,
                    freshness,
                    ..Default::default()
                })
                .register_source(MarkdownProvider::new(directory.clone()))
                .unwrap()
                .register_format(MarkdownDecoder::default())
                .unwrap()
                .register_representation(StoredRepresentation::default())
                .unwrap()
                .register_representation(RenderedRepresentation::new(TextFormat::Markdown).unwrap())
                .unwrap()
                .register_operator(PrefixOperator::default())
                .unwrap()
                .register_operator(NativeTextOperator::new(TextMode::Word, false))
                .unwrap()
                .register_extractor(PublicationYearExtractor::default())
                .unwrap()
                .register_frontend(ArchiveFrontend::default())
                .unwrap()
                .open()
                .unwrap()
        };
        let mut direct = open(Freshness::MetadataOnOpen);
        let mut indexed = open(Freshness::MetadataOnOpen);
        assert_eq!(
            indexed
                .build_selected_derived_posting("archive", "example:publication_year")
                .unwrap(),
            5
        );
        for (minimum, expected_ids) in [(1850, vec!["b", "d"]), (1861, vec![])] {
            let request = serde_json::json!({
                "collection":"archive", "min_year":minimum, "count":true,
            });
            let scan = direct
                .query_frontend("example:archive-search", &request)
                .unwrap();
            let result = indexed
                .query_frontend("example:archive-search", &request)
                .unwrap();
            assert_eq!(result.query_plan, ["extension-index-candidates"]);
            assert_eq!(result.identity, scan.identity);
            assert_eq!(result.total, scan.total);
            assert_eq!(result.truncated, scan.truncated);
            assert_eq!(result.metrics.fts_candidates, 0);
            assert_eq!(
                result
                    .rows
                    .iter()
                    .map(|row| row.source.record_id.as_str())
                    .collect::<Vec<_>>(),
                expected_ids
            );
            assert_eq!(
                result
                    .rows
                    .iter()
                    .map(|row| (&row.source.record_id, &row.values))
                    .collect::<Vec<_>>(),
                scan.rows
                    .iter()
                    .map(|row| (&row.source.record_id, &row.values))
                    .collect::<Vec<_>>()
            );
            assert!(result.metrics.source_reads < scan.metrics.source_reads);
        }
        let request = serde_json::json!({
            "collection":"archive", "min_year":1850, "sort_year":true,
            "count":true,
        });
        assert!(
            indexed
                .query_frontend("example:archive-search", &request)
                .is_err()
        );
        assert!(
            direct
                .query_frontend("example:archive-search", &request)
                .is_err()
        );
        let mut cached = Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(directory.clone()))
            .unwrap()
            .register_format(MarkdownDecoder::default())
            .unwrap()
            .register_representation(StoredRepresentation::default())
            .unwrap()
            .register_representation(RenderedRepresentation::new(TextFormat::Markdown).unwrap())
            .unwrap()
            .register_operator(PrefixOperator::default())
            .unwrap()
            .register_operator(NativeTextOperator::new(TextMode::Word, false))
            .unwrap()
            .register_extractor(PublicationYearExtractor::default())
            .unwrap()
            .register_frontend(ArchiveFrontend::default())
            .unwrap()
            .open()
            .unwrap();
        cached
            .build_selected_derived_posting("archive", "example:publication_year")
            .unwrap();
        cached
            .query_frontend(
                "example:archive-search",
                &serde_json::json!({
                    "collection":"archive", "min_year":1850,
                    "limit":1, "count":true,
                }),
            )
            .unwrap();
        let cached_page = cached
            .query_frontend(
                "example:archive-search",
                &serde_json::json!({
                    "collection":"archive", "min_year":1850,
                    "offset":1, "limit":1, "count":true,
                }),
            )
            .unwrap();
        assert_eq!(cached_page.query_plan, ["extension-ordered-cache"]);
        assert_eq!(cached_page.rows[0].source.record_id, "d");
        assert_eq!(cached_page.metrics.source_reads, 1);
        assert_eq!(cached_page.metrics.enumerations, 0);
        fs::write(
            directory.join("b.json"),
            r#"{"body":"Entry b","publication_date":"1840"}"#,
        )
        .unwrap();
        indexed.refresh("archive").unwrap();
        let request = serde_json::json!({
            "collection":"archive", "min_year":1850, "count":true,
        });
        let after_refresh = indexed
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(after_refresh.query_plan, ["extension-exact-scan"]);
        assert_eq!(after_refresh.total, Some(1));
        indexed
            .build_selected_derived_posting("archive", "example:publication_year")
            .unwrap();
        let rebuilt = indexed
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(rebuilt.query_plan, ["extension-index-candidates"]);
        assert_eq!(rebuilt.total, after_refresh.total);
        let mut on_request = open(Freshness::MetadataOnRequest);
        on_request
            .build_selected_derived_posting("archive", "example:publication_year")
            .unwrap();
        fs::write(
            directory.join("a.json"),
            r#"{"body":"Entry a","publication_date":"1863"}"#,
        )
        .unwrap();
        let changed_without_refresh = on_request
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(changed_without_refresh.query_plan, ["extension-exact-scan"]);
        assert_eq!(changed_without_refresh.total, Some(2));
        fs::write(
            directory.join("b.json"),
            r#"{"body":"Entry b","publication_date":"1848–1850"}"#,
        )
        .unwrap();
        indexed.refresh("archive").unwrap();
        assert!(
            indexed
                .build_selected_derived_posting("archive", "example:publication_year")
                .is_err()
        );
        assert!(
            indexed
                .query_frontend("example:archive-search", &request)
                .is_err()
        );
    }

    #[test]
    fn mixed_text_and_derived_or_requires_both_postings() {
        struct MixedOrFrontend(Descriptor);


        impl Default for MixedOrFrontend {
            fn default() -> Self {
                Self(Descriptor::new(
                    "example:mixed-or",
                    SemanticVersion {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    },
                    &(),
                    DataKind::Query,
                    DataKind::Query,
                    vec![],
                ))
            }
        }


        impl QueryFrontend for MixedOrFrontend {
            fn descriptor(&self) -> &Descriptor {
                &self.0
            }

            fn compile(&self, _: &Value, bindings: &ExtensionBindings<'_>) -> Result<LogicalPlan> {
                Ok(LogicalPlan {
                    collections: vec!["archive".into()],
                    predicate: LogicalPredicate::Or(vec![
                        LogicalPredicate::Call {
                            operator: bindings.operator("example:prefix").unwrap(),
                            representation: bindings.representation("objekte:stored").unwrap(),
                            path: "body".into(),
                            parameters: Value::String("target".into()),
                        },
                        LogicalPredicate::Derived {
                            extractor: bindings.extractor("example:publication_year").unwrap(),
                            predicate: objekte_core::DerivedPredicate::Ge(TypedValue::Number(
                                "1850".into(),
                            )),
                        },
                    ]),
                    sort: vec![],
                    columns: vec!["body".into()],
                    offset: 0,
                    limit: 10,
                    count: true,
                })
            }
        }


        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/archive");
        fs::create_dir_all(&directory).unwrap();
        for (id, body, date) in [
            ("a", "target A", "1800"),
            ("b", "other B", "1852"),
            ("c", "other C", "1800"),
        ] {
            fs::write(
                directory.join(format!("{id}.json")),
                serde_json::to_vec(&serde_json::json!({
                    "body":body, "publication_date":date,
                }))
                .unwrap(),
            )
            .unwrap();
        }
        fs::write(directory.join("d.md"), "target D").unwrap();
        let open = || {
            Repository::builder(temp.path())
                .options(Options {
                    query_cache_bytes: 0,
                    ..Default::default()
                })
                .register_source(MarkdownProvider::new(directory.clone()))
                .unwrap()
                .register_format(MarkdownDecoder::default())
                .unwrap()
                .register_representation(StoredRepresentation::default())
                .unwrap()
                .register_operator(PrefixOperator::default())
                .unwrap()
                .register_extractor(PublicationYearExtractor::default())
                .unwrap()
                .register_frontend(MixedOrFrontend::default())
                .unwrap()
                .open()
                .unwrap()
        };
        let mut direct = open();
        let mut indexed = open();
        indexed
            .build_representation_index("archive", "objekte:stored", "body")
            .unwrap();
        let request = Value::Null;
        let without_year_posting = indexed
            .query_frontend("example:mixed-or", &request)
            .unwrap();
        assert_eq!(without_year_posting.query_plan, ["extension-exact-scan"]);
        indexed
            .build_selected_derived_posting("archive", "example:publication_year")
            .unwrap();
        let expected = direct.query_frontend("example:mixed-or", &request).unwrap();
        let actual = indexed
            .query_frontend("example:mixed-or", &request)
            .unwrap();
        assert_eq!(actual.query_plan, ["extension-index-candidates"]);
        assert_eq!(actual.metrics.fts_candidates, 0);
        assert_eq!(actual.identity, expected.identity);
        assert_eq!(actual.total, expected.total);
        assert_eq!(actual.truncated, expected.truncated);
        assert_eq!(
            actual
                .rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "d"]
        );
        assert_eq!(
            actual
                .rows
                .iter()
                .map(|row| (&row.source.record_id, &row.values))
                .collect::<Vec<_>>(),
            expected
                .rows
                .iter()
                .map(|row| (&row.source.record_id, &row.values))
                .collect::<Vec<_>>()
        );
        assert!(actual.metrics.source_reads < expected.metrics.source_reads);
        let mut fused = open();
        fused.reset_metrics();
        assert_eq!(
            fused
                .build_selected_indexes(
                    "archive",
                    &[
                        objekte_core::SelectedIndexRequest::Text {
                            representation: "objekte:stored",
                            path: "body",
                        },
                        objekte_core::SelectedIndexRequest::Derived {
                            extractor: "example:publication_year",
                        },
                    ],
                )
                .unwrap(),
            4
        );
        assert_eq!(fused.metrics().source_reads, 4);
        assert_eq!(fused.metrics().parses, 4);
        let fused_result = fused.query_frontend("example:mixed-or", &request).unwrap();
        assert_eq!(fused_result.query_plan, actual.query_plan);
        assert_eq!(fused_result.identity, actual.identity);
        assert_eq!(
            fused_result
                .rows
                .iter()
                .map(|row| (&row.source.record_id, &row.values))
                .collect::<Vec<_>>(),
            actual
                .rows
                .iter()
                .map(|row| (&row.source.record_id, &row.values))
                .collect::<Vec<_>>()
        );
        fs::write(
            directory.join("c.json"),
            r#"{"body":"other C","publication_date":"1848–1850"}"#,
        )
        .unwrap();
        let mut failed = open();
        assert!(
            failed
                .build_selected_indexes(
                    "archive",
                    &[
                        objekte_core::SelectedIndexRequest::Text {
                            representation: "objekte:stored",
                            path: "body",
                        },
                        objekte_core::SelectedIndexRequest::Derived {
                            extractor: "example:publication_year",
                        },
                    ],
                )
                .is_err()
        );
        let previous_reads = failed.metrics().source_reads;
        failed
            .build_representation_index("archive", "objekte:stored", "body")
            .unwrap();
        assert_eq!(failed.metrics().source_reads - previous_reads, 4);
    }

    #[test]
    fn selected_derived_order_matches_heap_across_types_ties_and_pages() {
        struct SortFrontend(Descriptor);


        impl Default for SortFrontend {
            fn default() -> Self {
                Self(Descriptor::new(
                    "example:derived-sort",
                    SemanticVersion {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    },
                    &(),
                    DataKind::Query,
                    DataKind::Query,
                    vec![],
                ))
            }
        }


        impl QueryFrontend for SortFrontend {
            fn descriptor(&self) -> &Descriptor {
                &self.0
            }

            fn compile(&self, request: &Value, _: &ExtensionBindings<'_>) -> Result<LogicalPlan> {
                let direction = if request.get("desc").and_then(Value::as_bool) == Some(true) {
                    objekte_core::Direction::Desc
                } else {
                    objekte_core::Direction::Asc
                };
                Ok(LogicalPlan {
                    collections: vec!["archive".into()],
                    predicate: LogicalPredicate::And(Vec::new()),
                    sort: vec![objekte_core::Sort {
                        field: "@derived:example:publication_year".into(),
                        direction,
                    }],
                    columns: vec!["publication_date".into()],
                    offset: request.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
                    limit: request.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize,
                    count: true,
                })
            }
        }


        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/archive");
        fs::create_dir_all(&directory).unwrap();
        for (id, date) in [
            ("a", Some(serde_json::json!("1860"))),
            ("b", Some(serde_json::json!("1848"))),
            ("c", Some(serde_json::Value::Null)),
            ("d", None),
            ("e", Some(serde_json::json!("circa 1848"))),
            ("g", Some(serde_json::json!("1852"))),
            ("h", Some(serde_json::json!([]))),
        ] {
            let mut value = serde_json::json!({"body":format!("Entry {id}")});
            if let Some(date) = date {
                value["publication_date"] = date;
            }
            fs::write(
                directory.join(format!("{id}.json")),
                serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
        }
        fs::write(directory.join("f.md"), "No date").unwrap();
        let open = |cache: bool, total_limit: usize| {
            Repository::builder(temp.path())
                .options(Options {
                    query_cache_bytes: if cache { 64 * 1024 * 1024 } else { 0 },
                    total_limit,
                    ..Default::default()
                })
                .register_source(MarkdownProvider::new(directory.clone()))
                .unwrap()
                .register_format(MarkdownDecoder::default())
                .unwrap()
                .register_extractor(PublicationYearExtractor::default())
                .unwrap()
                .register_frontend(SortFrontend::default())
                .unwrap()
                .open()
                .unwrap()
        };
        let mut direct = open(false, 10_000);
        let mut indexed = open(false, 10_000);
        indexed
            .build_selected_derived_posting("archive", "example:publication_year")
            .unwrap();
        for (desc, expected_ids) in [
            (false, vec!["b", "e", "g", "a", "c", "d", "f", "h"]),
            (true, vec!["a", "g", "b", "e", "c", "d", "f", "h"]),
        ] {
            for (offset, limit) in [(0, 10), (2, 3), (6, 2)] {
                let request = serde_json::json!({
                    "desc":desc, "offset":offset, "limit":limit,
                });
                let scan = direct
                    .query_frontend("example:derived-sort", &request)
                    .unwrap();
                let sorted = indexed
                    .query_frontend("example:derived-sort", &request)
                    .unwrap();
                assert_eq!(scan.query_plan, ["extension-exact-scan"]);
                assert_eq!(sorted.query_plan, ["extension-indexed-order"]);
                assert_eq!(sorted.identity, scan.identity);
                assert_eq!(sorted.total, scan.total);
                assert_eq!(sorted.truncated, scan.truncated);
                assert_eq!(
                    sorted
                        .rows
                        .iter()
                        .map(|row| row.source.record_id.as_str())
                        .collect::<Vec<_>>(),
                    expected_ids[offset..expected_ids.len().min(offset + limit)]
                );
                assert_eq!(
                    sorted
                        .rows
                        .iter()
                        .map(|row| (&row.source.record_id, &row.values))
                        .collect::<Vec<_>>(),
                    scan.rows
                        .iter()
                        .map(|row| (&row.source.record_id, &row.values))
                        .collect::<Vec<_>>()
                );
            }
        }
        let mut cached = open(true, 10_000);
        cached
            .build_selected_derived_posting("archive", "example:publication_year")
            .unwrap();
        let first = cached
            .query_frontend("example:derived-sort", &serde_json::json!({"limit":1}))
            .unwrap();
        assert_eq!(first.query_plan, ["extension-indexed-order"]);
        let page = cached
            .query_frontend(
                "example:derived-sort",
                &serde_json::json!({"offset":2, "limit":2}),
            )
            .unwrap();
        assert_eq!(page.query_plan, ["extension-ordered-cache"]);
        assert_eq!(page.metrics.enumerations, 0);
        assert_eq!(page.metrics.source_reads, 2);
        assert_eq!(
            page.rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>(),
            ["g", "a"]
        );
        let mut capped_scan = open(false, 3);
        let mut capped_indexed = open(false, 3);
        capped_indexed
            .build_selected_derived_posting("archive", "example:publication_year")
            .unwrap();
        let request = serde_json::json!({"limit":3});
        let expected = capped_scan
            .query_frontend("example:derived-sort", &request)
            .unwrap();
        let actual = capped_indexed
            .query_frontend("example:derived-sort", &request)
            .unwrap();
        assert_eq!(actual.total, expected.total);
        assert!(actual.truncated);
        assert_eq!(actual.query_plan, ["extension-indexed-order"]);
        assert_eq!(
            actual
                .rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>(),
            ["b", "e", "g"]
        );
        assert_eq!(
            actual
                .rows
                .iter()
                .map(|row| (&row.source.record_id, &row.values))
                .collect::<Vec<_>>(),
            expected
                .rows
                .iter()
                .map(|row| (&row.source.record_id, &row.values))
                .collect::<Vec<_>>()
        );
        let snapshot = temp.path().join("selected-derived.snapshot");
        indexed
            .save_selected_derived_snapshot("archive", "example:publication_year", &snapshot)
            .unwrap();
        let mut restored = open(false, 10_000);
        assert!(restored
            .restore_selected_derived_snapshot(
                "archive",
                "example:publication_year",
                &snapshot,
            )
            .unwrap());
        assert_eq!(restored.metrics().source_reads, 0);
        assert_eq!(restored.metrics().parses, 0);
        let result = restored
            .query_frontend("example:derived-sort", &serde_json::json!({"limit":10}))
            .unwrap();
        assert_eq!(result.query_plan, ["extension-indexed-order"]);
        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>(),
            ["b", "e", "g", "a", "c", "d", "f", "h"]
        );
        let corrupted = temp.path().join("selected-derived-corrupted.snapshot");
        let mut bytes = fs::read(&snapshot).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&corrupted, bytes).unwrap();
        let mut rejected = open(false, 10_000);
        assert!(
            !rejected
                .restore_selected_derived_snapshot(
                    "archive",
                    "example:publication_year",
                    &corrupted,
                )
                .unwrap()
        );
        let mut changed_extractor = PublicationYearExtractor::default();
        changed_extractor.descriptor.version.patch += 1;
        let mut changed_semantics = Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(directory.clone()))
            .unwrap()
            .register_format(MarkdownDecoder::default())
            .unwrap()
            .register_extractor(changed_extractor)
            .unwrap()
            .register_frontend(SortFrontend::default())
            .unwrap()
            .open()
            .unwrap();
        assert!(!changed_semantics
            .restore_selected_derived_snapshot(
                "archive",
                "example:publication_year",
                &snapshot,
            )
            .unwrap());
        fs::write(
            directory.join("a.json"),
            r#"{"body":"Entry a","publication_date":"1800"}"#,
        )
        .unwrap();
        let mut stale = open(false, 10_000);
        assert!(!stale
            .restore_selected_derived_snapshot(
                "archive",
                "example:publication_year",
                &snapshot,
            )
            .unwrap());
        indexed.refresh("archive").unwrap();
        let request = serde_json::json!({"limit":10});
        let fallback = indexed
            .query_frontend("example:derived-sort", &request)
            .unwrap();
        assert_eq!(fallback.query_plan, ["extension-exact-scan"]);
        let mut fresh = open(false, 10_000);
        let oracle = fresh
            .query_frontend("example:derived-sort", &request)
            .unwrap();
        assert_eq!(
            fallback
                .rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>(),
            oracle
                .rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>()
        );
        indexed
            .build_selected_derived_posting("archive", "example:publication_year")
            .unwrap();
        let rebuilt = indexed
            .query_frontend("example:derived-sort", &request)
            .unwrap();
        assert_eq!(rebuilt.query_plan, ["extension-indexed-order"]);
        assert_eq!(
            rebuilt
                .rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>(),
            oracle
                .rows
                .iter()
                .map(|row| row.source.record_id.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn fallible_operator_hint_does_not_skip_an_erroring_record() {
        struct FallibleOperator(Descriptor);


        struct FallibleCompiled;


        impl CompiledOperator for FallibleCompiled {
            fn matches(&self, value: ScalarValue<'_>, _: &mut String) -> Result<bool> {
                let ScalarValue::Text(text) = value else {
                    return Ok(false);
                };
                if text == "danger" {
                    return Err(Error::Request("operator failed on danger".into()));
                }
                Ok(text.starts_with("target"))
            }

            fn candidate(&self) -> CandidateConstraint {
                CandidateConstraint::BigramLiteral {
                    literal: "target".into(),
                    representation: "objekte:stored".into(),
                }
            }
        }


        impl QueryOperator for FallibleOperator {
            fn descriptor(&self) -> &Descriptor {
                &self.0
            }

            fn compile(&self, _: &Value) -> Result<Box<dyn CompiledOperator>> {
                Ok(Box::new(FallibleCompiled))
            }
        }


        struct FallibleFrontend(Descriptor);


        impl QueryFrontend for FallibleFrontend {
            fn descriptor(&self) -> &Descriptor {
                &self.0
            }

            fn compile(
                &self,
                request: &Value,
                bindings: &ExtensionBindings<'_>,
            ) -> Result<LogicalPlan> {
                let call = |operator: &str, parameters: Value| LogicalPredicate::Call {
                    operator: bindings.operator(operator).unwrap(),
                    representation: bindings.representation("objekte:stored").unwrap(),
                    path: "body".into(),
                    parameters,
                };
                let fallible = call("example:fallible", Value::Null);
                let predicate = match request.as_str() {
                    Some("and") => LogicalPredicate::And(vec![
                        call("example:prefix", Value::String("target".into())),
                        fallible,
                    ]),
                    Some("or") => LogicalPredicate::Or(vec![
                        call("example:prefix", Value::String("danger".into())),
                        fallible,
                    ]),
                    Some("or-error") => LogicalPredicate::Or(vec![
                        call("example:prefix", Value::String("missing".into())),
                        fallible,
                    ]),
                    _ => fallible,
                };
                Ok(LogicalPlan {
                    collections: vec!["docs".into()],
                    predicate,
                    sort: vec![],
                    columns: vec!["body".into()],
                    offset: 0,
                    limit: 10,
                    count: true,
                })
            }
        }


        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/docs");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("a.json"), r#"{"body":"danger"}"#).unwrap();
        fs::write(directory.join("b.json"), r#"{"body":"target"}"#).unwrap();
        let descriptor = |identity, input, output| {
            Descriptor::new(
                identity,
                SemanticVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                &(),
                input,
                output,
                vec![Dependency::SourceBytes],
            )
        };
        for budget in [0, 64 * 1024 * 1024] {
            let mut repository = Repository::builder(temp.path())
                .options(Options {
                    query_cache_bytes: budget,
                    stream_query_cache: true,
                    ..Default::default()
                })
                .register_representation(StoredRepresentation::default())
                .unwrap()
                .register_operator(FallibleOperator(descriptor(
                    "example:fallible",
                    DataKind::Text,
                    DataKind::Predicate,
                )))
                .unwrap()
                .register_operator(PrefixOperator::default())
                .unwrap()
                .register_frontend(FallibleFrontend(descriptor(
                    "example:fallible-frontend",
                    DataKind::Query,
                    DataKind::Query,
                )))
                .unwrap()
                .open()
                .unwrap();
            if budget > 0 {
                repository
                    .build_representation_index("docs", "objekte:stored", "body")
                    .unwrap();
            }
            let query = TextQuery {
                pattern: "target".into(),
                ..Default::default()
            };
            for _ in 0..2 {
                assert!(matches!(
                    repository.query_frontend("example:fallible-frontend", &Value::Null),
                    Err(Error::Request(message)) if message == "operator failed on danger"
                ));
                assert!(matches!(
                    repository.search_text_with(&query, "objekte:stored", "example:fallible"),
                    Err(Error::Request(message)) if message == "operator failed on danger"
                ));
                assert!(matches!(
                    repository.search_text_stream_with(
                        &query,
                        "objekte:stored",
                        "example:fallible",
                        |_, _| Ok(())
                    ),
                    Err(Error::Request(message)) if message == "operator failed on danger"
                ));
            }
            let and_result = repository
                .query_frontend("example:fallible-frontend", &Value::String("and".into()))
                .unwrap();
            assert_eq!(and_result.total, Some(1));
            if budget > 0 {
                assert_eq!(and_result.query_plan, ["extension-index-candidates"]);
                assert_eq!(and_result.metrics.source_reads, 2);
            }
            assert_eq!(
                repository
                    .query_frontend("example:fallible-frontend", &Value::String("or".into()))
                    .unwrap()
                    .total,
                Some(2)
            );
            assert!(matches!(
                repository.query_frontend("example:fallible-frontend", &Value::String("or-error".into())),
                Err(Error::Request(message)) if message == "operator failed on danger"
            ));
        }
    }

    #[test]
    fn representation_postings_preserve_unicode_and_array_value_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/archive");
        fs::create_dir_all(directory.join(".editor")).unwrap();
        fs::write(
            directory.join(".editor/schema.json"),
            r#"{"type":"object","properties":{"contents":{"type":"array","items":{"type":"object","properties":{"text":{"type":"string","x-full-text":"markdown"}}}}}}"#,
        )
        .unwrap();
        for (id, values) in [
            ("a", vec!["Straße", "other"]),
            ("b", vec!["**Łódź**"]),
            ("c", vec!["foo", "bar"]),
        ] {
            fs::write(
                directory.join(format!("{id}.json")),
                serde_json::to_vec(&serde_json::json!({
                    "contents": values
                        .into_iter()
                        .map(|text| serde_json::json!({"text": text}))
                        .collect::<Vec<_>>()
                }))
                .unwrap(),
            )
            .unwrap();
        }
        for encoding in [
            objekte_core::BigramEncoding::Ascii,
            objekte_core::BigramEncoding::Utf8Bytes,
        ] {
            let open = || {
                Repository::builder(temp.path())
                    .options(Options {
                        bigram_encoding: encoding,
                        query_cache_bytes: 0,
                        ..Default::default()
                    })
                    .register_representation(StoredRepresentation::default())
                    .unwrap()
                    .register_representation(
                        RenderedRepresentation::new(TextFormat::Markdown).unwrap(),
                    )
                    .unwrap()
                    .register_operator(PrefixOperator::default())
                    .unwrap()
                    .register_operator(NativeTextOperator::new(TextMode::Word, false))
                    .unwrap()
                    .register_extractor(PublicationYearExtractor::default())
                    .unwrap()
                    .register_frontend(ArchiveFrontend::default())
                    .unwrap()
                    .open()
                    .unwrap()
            };
            let mut scan = open();
            let mut indexed = open();
            indexed
                .build_representation_index(
                    "archive",
                    "objekte:visible-markdown",
                    "contents[*].text",
                )
                .unwrap();
            for (term, expected_ids) in [
                ("STRASSE", vec!["a"]),
                ("łódź", vec!["b"]),
                ("foobar", vec![]),
            ] {
                let request = serde_json::json!({
                    "collection":"archive", "term_path":"contents[*].text",
                    "term":term, "count":true,
                });
                let expected = scan
                    .query_frontend("example:archive-search", &request)
                    .unwrap();
                let actual = indexed
                    .query_frontend("example:archive-search", &request)
                    .unwrap();
                let ids = |result: &objekte_core::ExtensionQueryResult| {
                    result
                        .rows
                        .iter()
                        .map(|row| row.source.record_id.clone())
                        .collect::<Vec<_>>()
                };
                assert_eq!(ids(&actual), expected_ids, "{encoding:?} {term}");
                assert_eq!(ids(&actual), ids(&expected));
                assert_eq!(actual.total, expected.total);
                assert_eq!(
                    actual.query_plan,
                    [
                        if term == "łódź" && encoding == objekte_core::BigramEncoding::Ascii {
                            "extension-exact-scan"
                        } else {
                            "extension-index-candidates"
                        }
                    ],
                    "{encoding:?} {term}"
                );
            }
        }
    }

    #[test]
    fn registered_markdown_provider_decoder_tracks_lifecycle_and_collisions() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/docs");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("seed.json"), r#"{"body":"json"}"#).unwrap();
        fs::write(directory.join("item.md"), "# Heading\nbody").unwrap();
        let repository = Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(directory.clone()))
            .unwrap()
            .register_format(MarkdownDecoder::default())
            .unwrap()
            .open()
            .unwrap();
        let descriptors = repository.source_descriptors("docs").unwrap();
        assert_eq!(
            descriptors
                .iter()
                .map(|source| source.record_id.as_str())
                .collect::<Vec<_>>(),
            ["item", "seed"]
        );
        assert_eq!(
            repository.read_source_leaves(&descriptors[0]).unwrap(),
            vec![objekte_core::DecodedLeaf {
                path: "/body".into(),
                value: TypedValue::Text("# Heading\nbody".into())
            }]
        );
        let body_path = objekte_core::JsonFilter::parse_path("body").unwrap();
        let body_length = repository
            .with_source_record(&descriptors[0], |record| {
                assert!(record.path_present(&body_path)?);
                let mut length = 0;
                record.visit_path(&body_path, &mut |value| {
                    if let ScalarValue::Text(text) = value {
                        length = text.len();
                    }
                    Ok(())
                })?;
                Ok(length)
            })
            .unwrap();
        assert_eq!(body_length, "# Heading\nbody".len());
        assert_eq!(
            repository.read_source_leaves(&descriptors[1]).unwrap()[0].value,
            TypedValue::Text("json".into())
        );

        fs::write(directory.join("new.md"), "new").unwrap();
        assert_eq!(repository.source_descriptors("docs").unwrap().len(), 3);
        fs::rename(directory.join("new.md"), directory.join("renamed.md")).unwrap();
        fs::remove_file(directory.join("item.md")).unwrap();
        assert_eq!(
            repository
                .source_descriptors("docs")
                .unwrap()
                .iter()
                .map(|source| source.record_id.as_str())
                .collect::<Vec<_>>(),
            ["renamed", "seed"]
        );
        assert!(repository.read_source_leaves(&descriptors[0]).is_err());

        fs::write(directory.join("renamed.json"), "{}").unwrap();
        assert!(
            matches!(repository.source_descriptors("docs"), Err(Error::Request(message)) if message.contains("collision"))
        );
    }

    #[test]
    fn selected_sources_discover_a_collection_without_json_files() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/docs");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("only.md"), "# Only").unwrap();
        let mut repository = Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(directory))
            .unwrap()
            .register_format(MarkdownDecoder::default())
            .unwrap()
            .open()
            .unwrap();
        let descriptors = repository.source_descriptors("docs").unwrap();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].record_id, "only");
        assert_eq!(
            repository.read_source_leaves(&descriptors[0]).unwrap()[0].value,
            TypedValue::Text("# Only".into())
        );
        let query = TextQuery {
            pattern: "Only".into(),
            collections: Some(vec!["docs".into()]),
            ..Default::default()
        };
        assert!(
            matches!(repository.search_text(&query), Err(Error::Request(message)) if message.contains("not yet included"))
        );
    }

    #[test]
    fn configuration_reload_tracks_custom_only_collection_membership() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/docs");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("only.md"), "# Only").unwrap();
        let mut repository = Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(directory.clone()))
            .unwrap()
            .register_format(MarkdownDecoder::default())
            .unwrap()
            .open()
            .unwrap();
        assert!(repository.model().collections.contains_key("docs"));
        fs::remove_file(directory.join("only.md")).unwrap();
        repository.reload().unwrap();
        assert!(!repository.model().collections.contains_key("docs"));
        fs::write(directory.join("new.md"), "new").unwrap();
        repository.reload().unwrap();
        assert_eq!(
            repository.source_descriptors("docs").unwrap()[0].record_id,
            "new"
        );
    }

    #[test]
    fn metadata_on_request_rejects_new_provider_sources_before_legacy_query() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/docs");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("seed.json"), r#"{"body":"json"}"#).unwrap();
        let options = Options {
            freshness: Freshness::MetadataOnRequest,
            ..Default::default()
        };
        let mut repository = Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(directory.clone()))
            .unwrap()
            .register_format(MarkdownDecoder::default())
            .unwrap()
            .options(options)
            .open()
            .unwrap();
        let query = TextQuery {
            pattern: "json".into(),
            collections: Some(vec!["docs".into()]),
            mode: TextMode::WordRaw,
            ..Default::default()
        };
        assert_eq!(repository.search_text(&query).unwrap().hits.len(), 1);
        fs::write(directory.join("item.md"), "# New").unwrap();
        assert!(
            matches!(repository.search_text(&query), Err(Error::Request(message)) if message.contains("not yet included"))
        );
        fs::remove_file(directory.join("item.md")).unwrap();
        assert_eq!(repository.search_text(&query).unwrap().hits.len(), 1);
    }

    #[test]
    fn custom_only_collection_can_bind_a_json_schema_to_logical_fields() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/docs");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("only.md"), "# Only").unwrap();
        let schema_dir = temp.path().join(".objekte/collections");
        fs::create_dir_all(&schema_dir).unwrap();
        fs::write(schema_dir.join("docs.schema.json"), r#"{
          "type":"object",
          "properties":{"body":{"type":"string","x-full-text":"markdown"}},
          "x-objekte":{"name":"docs","path":"data/docs","type":"data","format":"example:markdown","id":"filename"}
        }"#).unwrap();
        let repository = Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(directory))
            .unwrap()
            .register_format(MarkdownDecoder::default())
            .unwrap()
            .open()
            .unwrap();
        assert!(repository.model().collections["docs"].schema_path.is_some());
        let descriptors = repository.source_descriptors("docs").unwrap();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].format, "example:markdown");
    }

    #[test]
    fn registered_markdown_decoder_rejects_invalid_utf8_and_stale_revision() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/docs");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("seed.json"), "{}").unwrap();
        fs::write(directory.join("item.md"), [0xff]).unwrap();
        let repository = Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(directory.clone()))
            .unwrap()
            .register_format(MarkdownDecoder::default())
            .unwrap()
            .open()
            .unwrap();
        let descriptor = repository.source_descriptors("docs").unwrap().remove(0);
        assert!(
            matches!(repository.read_source_leaves(&descriptor), Err(Error::Request(message)) if message.contains("UTF-8"))
        );
        fs::write(directory.join("item.md"), "a different size").unwrap();
        assert!(repository.read_source_leaves(&descriptor).is_err());
    }

    #[test]
    fn markdown_decoder_lends_structured_presence_without_a_json_tree() {
        let body = objekte_core::JsonFilter::parse_path("body").unwrap();
        let absent = objekte_core::JsonFilter::parse_path("missing").unwrap();
        objekte_core::extensions::format::FormatDecoder::decode(
            &MarkdownDecoder::default(),
            b"",
            &mut |record| {
                assert!(record.path_present(&body)?);
                assert!(!record.path_present(&absent)?);
                let mut values = Vec::new();
                assert_eq!(
                    record.visit_path(&body, &mut |value| {
                        if let ScalarValue::Text(text) = value {
                            values.push(text.to_owned());
                        }
                        Ok(())
                    })?,
                    1
                );
                assert_eq!(values, [""]);
                Ok(())
            },
        )
        .unwrap();
    }

    #[test]
    fn overlapping_format_acceptors_are_rejected() {
        struct OverlappingDecoder(Descriptor);


        impl objekte_core::extensions::format::FormatDecoder for OverlappingDecoder {
            fn descriptor(&self) -> &Descriptor {
                &self.0
            }

            fn accepts(&self, source: &objekte_core::extensions::SourceDescriptor) -> bool {
                source.format == "example:markdown"
            }

            fn decode(
                &self,
                _: &[u8],
                _: &mut dyn FnMut(&dyn DecodedRecord) -> Result<()>,
            ) -> Result<()> {
                Ok(())
            }
        }


        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/docs");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("seed.json"), "{}").unwrap();
        fs::write(directory.join("item.md"), "# One").unwrap();
        let result = Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(directory))
            .unwrap()
            .register_format(MarkdownDecoder::default())
            .unwrap()
            .register_format(OverlappingDecoder(Descriptor::new(
                "example:other-markdown",
                SemanticVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                &(),
                DataKind::Source,
                DataKind::Record,
                vec![Dependency::SourceBytes],
            )))
            .unwrap()
            .open();
        assert!(matches!(result, Err(Error::Request(message)) if message.contains("ambiguous")));
    }

    #[test]
    fn decoder_callback_failure_and_extra_record_never_publish_a_result() {
        struct DoubleDecoder(Descriptor);


        impl objekte_core::extensions::format::FormatDecoder for DoubleDecoder {
            fn descriptor(&self) -> &Descriptor {
                &self.0
            }

            fn accepts(&self, source: &objekte_core::extensions::SourceDescriptor) -> bool {
                source.format == self.0.identity
            }

            fn decode(
                &self,
                bytes: &[u8],
                visit: &mut dyn FnMut(&dyn DecodedRecord) -> Result<()>,
            ) -> Result<()> {
                let body = std::str::from_utf8(bytes).unwrap();
                let record = MarkdownRecord { body };
                visit(&record)?;
                visit(&record)
            }
        }


        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/docs");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("seed.json"), "{}").unwrap();
        fs::write(directory.join("item.md"), "# One").unwrap();
        let repository = Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(directory))
            .unwrap()
            .register_format(DoubleDecoder(Descriptor::new(
                "example:markdown",
                SemanticVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                &(),
                DataKind::Source,
                DataKind::Record,
                vec![Dependency::SourceBytes],
            )))
            .unwrap()
            .open()
            .unwrap();
        let descriptor = repository.source_descriptors("docs").unwrap().remove(0);
        let error = repository.with_source_record(&descriptor, |_| {
            Err::<(), _>(Error::Request("cancelled".into()))
        });
        assert!(matches!(error, Err(Error::Request(message)) if message == "cancelled"));
        let error = repository.read_source_leaves(&descriptor);
        assert!(
            matches!(error, Err(Error::Request(message)) if message.contains("multiple records"))
        );
    }

    #[test]
    fn frontend_request_freshness_tracks_mixed_source_edits_and_caps_pages() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/archive");
        fs::create_dir_all(directory.join(".editor")).unwrap();
        fs::write(
            directory.join(".editor/schema.json"),
            r#"{"type":"object","properties":{"body":{"type":"string","x-full-text":"markdown"}}}"#,
        )
        .unwrap();
        fs::write(directory.join("a.json"), r#"{"body":"History A"}"#).unwrap();
        fs::write(directory.join("m.md"), "# History M").unwrap();
        let mut repository = Repository::builder(temp.path())
            .options(Options {
                freshness: Freshness::MetadataOnRequest,
                total_limit: 1,
                ..Default::default()
            })
            .register_source(MarkdownProvider::new(directory.clone()))
            .unwrap()
            .register_format(MarkdownDecoder::default())
            .unwrap()
            .register_representation(StoredRepresentation::default())
            .unwrap()
            .register_representation(RenderedRepresentation::new(TextFormat::Markdown).unwrap())
            .unwrap()
            .register_operator(PrefixOperator::default())
            .unwrap()
            .register_operator(NativeTextOperator::new(TextMode::Word, false))
            .unwrap()
            .register_extractor(PublicationYearExtractor::default())
            .unwrap()
            .register_frontend(ArchiveFrontend::default())
            .unwrap()
            .open()
            .unwrap();
        let request = serde_json::json!({
            "collection":"archive", "term":"history", "limit":1, "count":true,
        });
        let first = repository
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(first.total, Some(2));
        assert!(first.truncated);
        assert_eq!(first.rows[0].source.record_id, "a");
        assert_eq!(first.metrics.source_reads, 3);
        fs::write(directory.join("m.md"), "# Different M").unwrap();
        let after_edit = repository
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(after_edit.total, Some(1));
        assert!(!after_edit.truncated);
        let reused = repository
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(reused.query_plan, ["extension-ordered-cache"]);
        assert_eq!(reused.metrics.enumerations, 2);
        assert_eq!(reused.metrics.source_reads, 1);
        fs::rename(directory.join("m.md"), directory.join("renamed.md")).unwrap();
        fs::write(directory.join("a.json"), r#"{"body":"Novel A"}"#).unwrap();
        assert_eq!(
            repository
                .query_frontend("example:archive-search", &request)
                .unwrap()
                .total,
            Some(0)
        );
        fs::write(directory.join("new.md"), "# History N").unwrap();
        assert_eq!(
            repository
                .query_frontend("example:archive-search", &request)
                .unwrap()
                .rows[0]
                .source
                .record_id,
            "new"
        );
        fs::write(
            directory.join("new.json"),
            r#"{"body":"History collision"}"#,
        )
        .unwrap();
        assert!(matches!(
            repository.query_frontend("example:archive-search", &request),
            Err(Error::Request(message)) if message.contains("collision")
        ));
    }

    #[test]
    fn frontend_binds_array_markup_paths_and_stable_registration_identity() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/archive");
        fs::create_dir_all(directory.join(".editor")).unwrap();
        fs::write(
            directory.join(".editor/schema.json"),
            r#"{"type":"object","properties":{"contents":{"type":"array","items":{"type":"object","properties":{"text":{"type":"string","x-full-text":"markdown"}}}}}}"#,
        )
        .unwrap();
        fs::write(
            directory.join("a.json"),
            r#"{"contents":[{"text":"first"},{"text":"**world** two"}]}"#,
        )
        .unwrap();
        let mut first = Repository::builder(temp.path())
            .register_representation(StoredRepresentation::default())
            .unwrap()
            .register_representation(RenderedRepresentation::new(TextFormat::Markdown).unwrap())
            .unwrap()
            .register_operator(PrefixOperator::default())
            .unwrap()
            .register_operator(NativeTextOperator::new(TextMode::Word, false))
            .unwrap()
            .register_extractor(PublicationYearExtractor::default())
            .unwrap()
            .register_frontend(ArchiveFrontend::default())
            .unwrap()
            .open()
            .unwrap();
        let mut reordered = Repository::builder(temp.path())
            .register_frontend(ArchiveFrontend::default())
            .unwrap()
            .register_extractor(PublicationYearExtractor::default())
            .unwrap()
            .register_operator(NativeTextOperator::new(TextMode::Word, false))
            .unwrap()
            .register_operator(PrefixOperator::default())
            .unwrap()
            .register_representation(RenderedRepresentation::new(TextFormat::Markdown).unwrap())
            .unwrap()
            .register_representation(StoredRepresentation::default())
            .unwrap()
            .open()
            .unwrap();
        let request = serde_json::json!({
            "collection":"archive", "term":"world two",
            "term_path":"contents[*].text", "count":true,
        });
        let expected = first
            .query_frontend("example:archive-search", &request)
            .unwrap();
        let actual = reordered
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(expected.total, Some(1));
        assert_eq!(actual.total, expected.total);
        assert_eq!(actual.identity, expected.identity);
        assert_eq!(actual.rows[0].source.record_id, "a");
        let cached = first
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(cached.query_plan, ["extension-ordered-cache"]);
        assert_eq!(cached.metrics.enumerations, 0);
        fs::write(
            directory.join(".editor/schema.json"),
            r#"{"type":"object","properties":{"contents":{"type":"array","items":{"type":"object","properties":{"text":{"type":"string"}}}}}}"#,
        )
        .unwrap();
        first.reload().unwrap();
        let changed = first
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(changed.total, Some(0));
        assert_ne!(changed.identity, expected.identity);
    }

    #[test]
    fn frontend_open_freshness_freezes_initially_empty_provider_selection() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("data/archive");
        fs::create_dir_all(directory.join(".editor")).unwrap();
        fs::write(
            directory.join(".editor/schema.json"),
            r#"{"type":"object","properties":{"body":{"type":"string","x-full-text":"markdown"}}}"#,
        )
        .unwrap();
        fs::write(directory.join("a.json"), r#"{"body":"History A"}"#).unwrap();
        let mut repository = Repository::builder(temp.path())
            .register_source(MarkdownProvider::new(directory.clone()))
            .unwrap()
            .register_format(MarkdownDecoder::default())
            .unwrap()
            .register_representation(StoredRepresentation::default())
            .unwrap()
            .register_representation(RenderedRepresentation::new(TextFormat::Markdown).unwrap())
            .unwrap()
            .register_operator(PrefixOperator::default())
            .unwrap()
            .register_operator(NativeTextOperator::new(TextMode::Word, false))
            .unwrap()
            .register_extractor(PublicationYearExtractor::default())
            .unwrap()
            .register_frontend(ArchiveFrontend::default())
            .unwrap()
            .open()
            .unwrap();
        let request = serde_json::json!({
            "collection":"archive", "term":"History", "count":true,
        });
        assert_eq!(
            repository
                .query_frontend("example:archive-search", &request)
                .unwrap()
                .total,
            Some(1)
        );
        fs::write(directory.join("new.md"), "# History new").unwrap();
        let cached = repository
            .query_frontend("example:archive-search", &request)
            .unwrap();
        assert_eq!(cached.total, Some(1));
        assert_eq!(cached.metrics.enumerations, 0);
        repository.reload().unwrap();
        assert_eq!(
            repository
                .query_frontend("example:archive-search", &request)
                .unwrap()
                .total,
            Some(2)
        );
    }


    struct ExactNumberExtractor {
        descriptor: Descriptor,
        path: objekte_core::JsonPath,
    }


    impl Default for ExactNumberExtractor {
        fn default() -> Self {
            Self {
                descriptor: Descriptor::new(
                    "example:exact_number",
                    SemanticVersion {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    },
                    &"number",
                    DataKind::Record,
                    DataKind::TypedValues,
                    vec![Dependency::SourceBytes],
                ),
                path: objekte_core::JsonFilter::parse_path("number").unwrap(),
            }
        }
    }


    impl ValueExtractor for ExactNumberExtractor {
        fn descriptor(&self) -> &Descriptor {
            &self.descriptor
        }

        fn binds(&self, context: &BindingContext<'_>) -> bool {
            context.path == "@derived:example:exact_number"
        }

        fn extract(
            &self,
            record: &dyn RecordAccess,
            output: &mut Vec<TypedValue>,
        ) -> Result<Extraction> {
            if !record.path_present(&self.path)? {
                return Ok(Extraction::Missing);
            }
            record.visit_path(&self.path, &mut |value| {
                match value {
                    ScalarValue::Number(value) => output.push(TypedValue::Number(value.into())),
                    ScalarValue::Null => output.push(TypedValue::Null),
                    _ => return Err(Error::Request("number must be numeric or null".into())),
                }
                Ok(())
            })?;
            Ok(Extraction::Present)
        }
    }


    #[test]
    fn publication_year_grammar_and_presence_are_distinct() {
        let extractor = PublicationYearExtractor::default();
        for (document, expected, values) in [
            (serde_json::json!({}), Extraction::Missing, vec![]),
            (
                serde_json::json!({"publication_date": []}),
                Extraction::Present,
                vec![],
            ),
            (
                serde_json::json!({"publication_date": null}),
                Extraction::Present,
                vec![TypedValue::Null],
            ),
            (
                serde_json::json!({"publication_date": "circa 1848"}),
                Extraction::Present,
                vec![TypedValue::Number("1848".into())],
            ),
            (
                serde_json::json!({"publication_date": ["1848", "circa 1851"]}),
                Extraction::Present,
                vec![
                    TypedValue::Number("1848".into()),
                    TypedValue::Number("1851".into()),
                ],
            ),
        ] {
            let mut extracted = Vec::new();
            assert_eq!(
                extractor.extract(&document, &mut extracted).unwrap(),
                expected
            );
            assert_eq!(extracted, values);
        }
        for invalid in ["1848–1850", "1848-1850", "about 1848", "184", "18480"] {
            let document = serde_json::json!({"publication_date": invalid});
            assert!(matches!(
                extractor.extract(&document, &mut Vec::new()),
                Err(Error::Request(_))
            ));
        }
    }

    #[test]
    fn publication_year_posting_patches_overwritten_and_deleted_sources() {
        let root = tempfile::tempdir().unwrap();
        let books = root.path().join("data/books");
        fs::create_dir_all(&books).unwrap();
        for (id, value) in [
            ("a", r#"{"publication_date":"circa 1848"}"#),
            ("b", r#"{"publication_date":["1848","1851"]}"#),
            ("c", r#"{"publication_date":null}"#),
            ("d", r#"{"publication_date":[]}"#),
            ("e", r#"{}"#),
        ] {
            fs::write(books.join(format!("{id}.json")), value).unwrap();
        }
        let mut repository = Repository::builder(root.path())
            .register_extractor(PublicationYearExtractor::default())
            .unwrap()
            .open()
            .unwrap();
        let counts = repository
            .derived_value_counts("books", "example:publication_year")
            .unwrap();
        assert_eq!(
            counts,
            vec![
                (TypedValue::Number("1848".into()), 2),
                (TypedValue::Number("1851".into()), 1),
            ]
        );
        repository.save_bigram_cache("books").unwrap();
        let mut restored = Repository::builder(root.path())
            .register_extractor(PublicationYearExtractor::default())
            .unwrap()
            .open()
            .unwrap();
        assert_eq!(
            restored
                .derived_value_counts("books", "example:publication_year")
                .unwrap(),
            counts
        );
        fs::write(books.join("a.json"), r#"{"publication_date":"circa 1860"}"#).unwrap();
        fs::remove_file(books.join("b.json")).unwrap();
        let counts = repository
            .derived_value_counts("books", "example:publication_year")
            .unwrap();
        assert_eq!(counts, vec![(TypedValue::Number("1860".into()), 1)]);
        fs::write(books.join("a.json"), r#"{"publication_date":"1848–1850"}"#).unwrap();
        assert!(matches!(
            repository.derived_value_counts("books", "example:publication_year"),
            Err(Error::Request(_))
        ));
    }

    #[test]
    fn publication_year_direct_indexed_and_cached_pages_agree() {
        let root = tempfile::tempdir().unwrap();
        let books = root.path().join("data/books");
        fs::create_dir_all(&books).unwrap();
        for (id, value) in [
            ("a", r#"{"publication_date":"circa 1848"}"#),
            ("b", r#"{"publication_date":["1848","1851"]}"#),
            ("c", r#"{"publication_date":null}"#),
            ("d", r#"{"publication_date":[]}"#),
            ("e", r#"{}"#),
        ] {
            fs::write(books.join(format!("{id}.json")), value).unwrap();
        }
        let mut options = Options::new();
        options.freshness = Freshness::MetadataOnOpen;
        let mut repository = Repository::builder(root.path())
            .options(options)
            .register_extractor(PublicationYearExtractor::default())
            .unwrap()
            .open()
            .unwrap();
        let mut query = DerivedQuery {
            collection: "books".into(),
            extractor: "example:publication_year".into(),
            predicate: DerivedPredicate::AnyValue,
            sort: Some(DerivedSort {
                direction: Direction::Desc,
                reduce: SortReduce::Max,
            }),
            offset: 0,
            limit: 2,
            count: true,
            backend: DerivedBackend::Direct,
        };
        let direct = repository.query_derived(&query).unwrap();
        assert_eq!(direct.total, Some(3));
        assert_eq!(
            direct
                .records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            ["b", "a"]
        );
        query.backend = DerivedBackend::Indexed;
        let indexed = repository.query_derived(&query).unwrap();
        assert_eq!(indexed.total, direct.total);
        assert_eq!(
            indexed
                .records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            ["b", "a"]
        );
        query.offset = 2;
        assert_eq!(repository.query_derived(&query).unwrap().records[0].id, "c");
        repository.reset_metrics();
        assert_eq!(repository.query_derived(&query).unwrap().records[0].id, "c");
        assert_eq!(repository.metrics().enumerations, 0);
        query.predicate = DerivedPredicate::Gt(TypedValue::Number("1848".into()));
        query.offset = 0;
        for backend in [DerivedBackend::Direct, DerivedBackend::Indexed] {
            query.backend = backend;
            let result = repository.query_derived(&query).unwrap();
            assert_eq!(result.total, Some(1));
            assert_eq!(result.records[0].id, "b");
        }
    }

    #[test]
    fn derived_numbers_keep_exact_large_and_equal_decimal_spellings() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("data/numbers");
        fs::create_dir_all(&folder).unwrap();
        for (id, value) in [
            ("a", r#"{"number":9007199254740992}"#),
            ("b", r#"{"number":9007199254740993}"#),
            ("c", r#"{"number":1e100}"#),
            ("d", r#"{"number":1e+100}"#),
            ("e", r#"{"number":null}"#),
            ("f", r#"{}"#),
        ] {
            fs::write(folder.join(format!("{id}.json")), value).unwrap();
        }
        let mut repo = Repository::builder(root.path())
            .register_extractor(ExactNumberExtractor::default())
            .unwrap()
            .open()
            .unwrap();
        let mut query = DerivedQuery {
            collection: "numbers".into(),
            extractor: "example:exact_number".into(),
            predicate: DerivedPredicate::Eq(TypedValue::Number("1e100".into())),
            sort: None,
            offset: 0,
            limit: 10,
            count: true,
            backend: DerivedBackend::Direct,
        };
        let direct = repo.query_derived(&query).unwrap();
        assert_eq!(direct.total, Some(2));
        assert_eq!(
            direct
                .records
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["c", "d"]
        );
        query.backend = DerivedBackend::Indexed;
        let indexed = repo.query_derived(&query).unwrap();
        assert_eq!(indexed.total, direct.total);
        assert_eq!(
            indexed
                .records
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["c", "d"]
        );
        query.predicate = DerivedPredicate::Gt(TypedValue::Number("9007199254740992".into()));
        for backend in [DerivedBackend::Direct, DerivedBackend::Indexed] {
            query.backend = backend;
            let result = repo.query_derived(&query).unwrap();
            assert_eq!(
                result
                    .records
                    .iter()
                    .map(|r| r.id.as_str())
                    .collect::<Vec<_>>(),
                ["b", "c", "d"]
            );
        }
    }

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
