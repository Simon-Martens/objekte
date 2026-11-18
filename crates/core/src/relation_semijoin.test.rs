use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::model::{LogicalType, Value};
use crate::query::{CaseSensitivity, FtsMode, Query, QueryOptions, RegexOptions, parse_filter};
use crate::reference::{BoundExpr, FieldDefinition, Schema};
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::{CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions};
use crate::source::{
    ExtractedRecord, JsonReader, LogicalTextCoverage, ReaderFailure, ScanProjection, SourceFormat,
    SourceReader, SourceRegistry, TextDerivation,
};
use crate::workload::{BigramAlphabet, ReturnMode, WorkloadCase};

use super::{Strategy, candidates_with_strategy};

struct CountedJsonReader(Arc<AtomicUsize>);

impl SourceReader for CountedJsonReader {
    fn name(&self) -> &'static str {
        "counted_json"
    }

    fn text_derivation(&self) -> TextDerivation {
        JsonReader.text_derivation()
    }

    fn logical_text_coverage(&self) -> LogicalTextCoverage {
        JsonReader.logical_text_coverage()
    }

    fn frame(&self, bytes: &[u8]) -> Result<Vec<crate::inventory::ByteRange>, String> {
        JsonReader.frame(bytes)
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        self.0.fetch_add(1, Ordering::Relaxed);
        JsonReader.parse(bytes)
    }

    fn parse_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        self.0.fetch_add(1, Ordering::Relaxed);
        JsonReader.parse_outcome(bytes)
    }

    fn parse_for_build_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        self.0.fetch_add(1, Ordering::Relaxed);
        JsonReader.parse_for_build_outcome(bytes)
    }

    fn parse_for_query(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
    ) -> Result<ExtractedRecord, String> {
        self.0.fetch_add(1, Ordering::Relaxed);
        JsonReader.parse_for_query(bytes, projection)
    }
}

#[test]
fn estimate_only_explain_never_opens_or_parses_related_sources() {
    let root = std::env::temp_dir().join(format!(
        "objekte-explain-no-source-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for collection in ["sources", "targets"] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
    }
    std::fs::write(
        root.join("sources/S.json"),
        r#"{"id":"S","entry":"T","title":"Local"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("targets/T.json"),
        r#"{"id":"T","title":"Needle"}"#,
    )
    .unwrap();
    let parses = Arc::new(AtomicUsize::new(0));
    let mut registry = SourceRegistry::builtin();
    registry
        .register(
            "counted_json",
            1,
            "json",
            Arc::new(CountedJsonReader(parses.clone())),
        )
        .unwrap();
    let mut definitions = definitions();
    let title_path = vec!["title".to_owned()];
    definitions[1]
        .schema
        .fields
        .get_mut(&title_path)
        .unwrap()
        .indexed = false;
    definitions[1]
        .schema
        .prohibit_auto_index(title_path)
        .unwrap();
    let definitions = definitions
        .into_iter()
        .map(|mut definition| {
            definition.format = SourceFormat::registered("counted_json");
            definition
        })
        .collect();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..Default::default()
        },
        registry,
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let relation = query("entry.title ?~ 'Needle'");
    let related_fts = selected_fts(None, "Needle", false);
    // Any attempted source open now fails, even if the reader would skip parsing.
    std::fs::rename(root.join("sources/S.json"), root.join("sources/S.hidden")).unwrap();
    std::fs::rename(root.join("targets/T.json"), root.join("targets/T.hidden")).unwrap();
    let before_explain = parses.load(Ordering::Relaxed);
    for request in [&relation, &related_fts] {
        let traces = repo.explain(request).unwrap();
        assert_eq!(traces.len(), 1);
        assert!(traces[0].candidates_estimated);
        assert!(traces[0].sort_strategy_estimated);
        assert!(traces[0].actual.is_none());
        assert!(
            traces[0]
                .terms
                .iter()
                .all(|term| !term.executed && term.actual_output.is_none())
        );
    }
    assert_eq!(parses.load(Ordering::Relaxed), before_explain);
    std::fs::rename(root.join("sources/S.hidden"), root.join("sources/S.json")).unwrap();
    std::fs::rename(root.join("targets/T.hidden"), root.join("targets/T.json")).unwrap();
    let before_analyze = parses.load(Ordering::Relaxed);
    let (_, analyzed) = repo.explain_analyze(&relation).unwrap();
    assert!(analyzed[0].actual.as_ref().unwrap().relation_target_reads > 0);
    assert!(parses.load(Ordering::Relaxed) > before_analyze);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn intermediate_limits_report_errors_instead_of_truncating_membership() {
    assert!(super::ensure_slots(super::MAX_INTERMEDIATE_SLOTS + 1, None).is_err());
    let mut work = super::SemijoinWork {
        edge_visits: super::MAX_VISITED_EDGES,
        ..Default::default()
    };
    assert!(super::visit_edge(&mut work, None).is_err());
}

#[test]
fn second_target_in_batch_is_checked_before_source_read() {
    use crate::streaming::{ExecutionContext, ExecutionErrorKind, ExecutionLimits};
    let root = std::env::temp_dir().join(format!(
        "objekte-semijoin-batch-limit-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("sources")).unwrap();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    let first = b"{\"id\":\"T0\",\"title\":\"a\"}\n";
    let second = b"{\"id\":\"T1\",\"title\":\"a much longer title\"}\n";
    std::fs::write(
        root.join("targets/records.jsonl"),
        [first.as_slice(), second.as_slice()].concat(),
    )
    .unwrap();
    let mut definitions = definitions();
    definitions[1].format = SourceFormat::JsonLines;
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let target_index = repo
        .collections
        .iter()
        .position(|collection| collection.definition.name == "targets")
        .unwrap();
    let context = ExecutionContext::new(
        crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
        ExecutionLimits {
            target_record_bytes: first.len() as u64,
            ..Default::default()
        },
        None,
    );
    let mut work = super::SemijoinWork::default();
    let error = super::verify_target_records(
        &repo,
        target_index,
        &roaring::RoaringBitmap::from_iter([0, 1]),
        None,
        false,
        &mut work,
        Some(&context),
        &mut |_, _| Ok(true),
    )
    .unwrap_err();
    assert_eq!(
        context.recorded_error(error).kind,
        ExecutionErrorKind::Resource
    );
    assert_eq!(work.target_file_opens, 0);
    assert_eq!(work.target_reads, 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn relation_planning_limit_has_ordinary_and_stream_resource_class() {
    use crate::streaming::{
        ExecutionContext, ExecutionErrorKind, ExecutionLimits, ResultScope, StreamControl,
        StreamError, StreamOptions,
    };
    let (root, repo) = fixture(BigramAlphabet::Ascii);
    let request = query("entry.title ?~ 'Hymn'");
    let context = || {
        ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits {
                intermediate_slots: 0,
                ..Default::default()
            },
            None,
        )
    };
    assert_eq!(
        repo.search_with_context(&request, &context())
            .unwrap_err()
            .kind,
        ExecutionErrorKind::Resource
    );
    let stream = repo.search_stream_scoped_with_context(
        &request,
        ResultScope::Page {
            offset: 0,
            limit: request.options.limit,
        },
        StreamOptions::default(),
        &context(),
        &mut |_| Ok(StreamControl::Continue),
    );
    assert!(
        matches!(stream, Err(StreamError::Resource { .. })),
        "{stream:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn relation_edge_planning_and_target_verification_cancel_mid_work() {
    use crate::streaming::{
        CancellationToken, ExecutionContext, ExecutionErrorKind, ExecutionLimits,
    };
    let (root, repo) = fixture(BigramAlphabet::Ascii);
    let source_index = repo
        .collections
        .iter()
        .position(|item| item.definition.name == "sources")
        .unwrap();
    let target_index = repo
        .collections
        .iter()
        .position(|item| item.definition.name == "targets")
        .unwrap();
    let request = query("entry.title ?~ 'Hymn'");
    let bound = crate::relation_query::bind_filter_with_relations(
        &repo,
        source_index,
        request.filter.as_ref().unwrap(),
        &mut HashMap::new(),
    )
    .unwrap();
    let BoundExpr::Term(term) = bound else {
        panic!("expected relation term")
    };
    let hops = super::eligible(&repo, &term).unwrap();
    let context = |token: CancellationToken| {
        ExecutionContext::new(
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits::default(),
            Some(token),
        )
    };

    let edge_token = CancellationToken::new();
    let edge_context = context(edge_token.clone());
    edge_context.cancel_on_check_for_test(3);
    let mut edge_work = super::SemijoinWork::default();
    let error = super::reachable_targets(
        &repo,
        hops,
        &repo.collections[source_index].logical_records,
        &mut edge_work,
        Some(&edge_context),
    )
    .unwrap_err();
    assert!(edge_work.edge_visits > 0);
    assert!(edge_token.is_cancelled());
    assert_eq!(
        edge_context.recorded_error(error).kind,
        ExecutionErrorKind::Cancelled
    );

    let verify_token = CancellationToken::new();
    let verify_context = context(verify_token.clone());
    verify_context.cancel_on_check_for_test(15);
    let mut verify_work = super::SemijoinWork::default();
    let error = super::verify_target_records(
        &repo,
        target_index,
        &repo.collections[target_index].logical_records,
        Some("title"),
        false,
        &mut verify_work,
        Some(&verify_context),
        &mut |_, _| Ok(true),
    )
    .unwrap_err();
    assert!(verify_work.target_reads > 0);
    assert!(verify_token.is_cancelled());
    assert_eq!(
        verify_context.recorded_error(error).kind,
        ExecutionErrorKind::Cancelled
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn field(schema: &mut Schema, name: &str, kind: LogicalType) {
    schema
        .register_field(FieldDefinition {
            path: vec![name.into()],
            kind,
            required: false,
            nullable: true,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
}

fn definition(name: &str, schema: Schema) -> CollectionDefinition {
    CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
        discovery: Default::default(),
        schema,
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

fn definitions() -> Vec<CollectionDefinition> {
    let mut targets = Schema::default();
    field(&mut targets, "id", LogicalType::Id);
    field(&mut targets, "title", LogicalType::String);
    field(&mut targets, "related_entry", LogicalType::Id);
    targets
        .register_relation(RelationDefinition {
            path: vec!["related_entry".into()],
            target_collection: "targets".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("related_from".into()),
        })
        .unwrap();
    let mut sources = Schema::default();
    field(&mut sources, "id", LogicalType::Id);
    field(&mut sources, "entry", LogicalType::Id);
    field(&mut sources, "title", LogicalType::String);
    sources
        .register_relation(RelationDefinition {
            path: vec!["entry".into()],
            target_collection: "targets".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("sources".into()),
        })
        .unwrap();
    vec![
        definition("sources", sources),
        definition("targets", targets),
    ]
}

fn fixture(alphabet: BigramAlphabet) -> (std::path::PathBuf, Repository) {
    let root = std::env::temp_dir().join(format!(
        "objekte-semijoin-{}-{:?}-{}",
        std::process::id(),
        std::thread::current().id(),
        alphabet as u8,
    ));
    for name in ["sources", "targets"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    for index in 0..25 {
        let (title, link) = match index {
            0 => ("Hymn Parade", "T01"),
            1 => ("Festival", "T01"),
            _ => ("Other", "T01"),
        };
        std::fs::write(
            root.join(format!("targets/T{index:02}.json")),
            format!(r#"{{"id":"T{index:02}","title":"{title}","related_entry":"{link}"}}"#),
        )
        .unwrap();
    }
    for index in 0..60 {
        let link = if index < 40 {
            "T00".to_owned()
        } else if index == 59 {
            "missing".to_owned()
        } else {
            format!("T{:02}", (index - 39).min(24))
        };
        let title = if index == 0 { "rare" } else { "common" };
        std::fs::write(
            root.join(format!("sources/S{index:02}.json")),
            format!(r#"{{"id":"S{index:02}","entry":"{link}","title":"{title}"}}"#),
        )
        .unwrap();
    }
    std::fs::write(root.join("sources/broken.json"), "{broken").unwrap();

    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions(),
        RepositoryOptions {
            bigram_alphabet: alphabet,
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    (root, repo)
}

fn query(filter: &str) -> Query {
    Query {
        collections: vec!["sources".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 100,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    }
}

fn selected_fts(filter: Option<&str>, literal: &str, regex: bool) -> Query {
    selected_fts_fields(filter, literal, regex, vec!["entry.title".into()])
}

fn selected_fts_fields(
    filter: Option<&str>,
    literal: &str,
    regex: bool,
    fields: Vec<String>,
) -> Query {
    Query::from_workload(&WorkloadCase {
        id: "relation-fts-semijoin".into(),
        collection: "sources".into(),
        fts: Some(literal.into()),
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: if regex {
            FtsMode::Regex(RegexOptions::default())
        } else {
            FtsMode::Literal
        },
        filter: filter.map(str::to_owned),
        fts_fields: fields,
        sort: vec![],
        limit: 100,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: true,
    })
    .unwrap()
}

fn ids(repo: &Repository, request: &Query) -> Vec<String> {
    let found = repo.search(request).unwrap();
    let reference = repo.search_reference(request).unwrap();
    assert_eq!(found.hits, reference.hits);
    assert_eq!(found.count, reference.count);
    found.hits.into_iter().map(|hit| hit.record_id).collect()
}

#[test]
fn relation_semijoin_distinguishes_repeated_local_slots_across_segments() {
    let root = std::env::temp_dir().join(format!(
        "objekte-semijoin-local-slots-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&root);
    for name in ["sources", "targets"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    std::fs::write(
        root.join("targets/T0.json"),
        r#"{"id":"T0","title":"Needle","related_entry":"T0"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("targets/T1.json"),
        r#"{"id":"T1","title":"Other","related_entry":"T0"}"#,
    )
    .unwrap();
    // Ordinary JSON files are extracted in batches of up to 512 files. Put the
    // colliding records on either side of that boundary so the public policy
    // genuinely produces two source segments.
    for number in 0..513 {
        let id = match number {
            0 => "S0".to_owned(),
            512 => "S1".to_owned(),
            _ => format!("F{number:03}"),
        };
        let (target, title) = match number {
            0 => ("T0", "Needle"),
            512 => ("T1", "Other"),
            _ => ("missing", "filler"),
        };
        std::fs::write(
            root.join(format!("sources/{number:03}.json")),
            format!(r#"{{"id":"{id}","entry":"{target}","title":"{title}"}}"#),
        )
        .unwrap();
    }

    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions(),
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(1).unwrap(),
                max_files: std::num::NonZeroUsize::new(1).unwrap(),
            },
            ..Default::default()
        },
        crate::source::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let source_index = repo
        .collections()
        .iter()
        .position(|collection| collection.definition.name == "sources")
        .unwrap();
    let source_segments = &repo.segments[source_index];
    assert_eq!(source_segments.len(), 2);
    assert_eq!(source_segments[0].index.records[0].slot.0, 0);
    assert_eq!(source_segments[1].index.records[0].slot.0, 0);

    let request = query("entry.title = 'Needle'");
    assert_eq!(ids(&repo, &request), ["S0"]);
    assert_eq!(repo.search(&request).unwrap().count, Some(1));
    let fts = selected_fts(None, "Needle", false);
    assert_eq!(ids(&repo, &fts), ["S0"]);
    let fts = repo.search(&fts).unwrap();
    assert_eq!(fts.count, Some(1));
    assert_eq!(fts.hits[0].matched_fields, ["entry.title"]);
    assert_eq!(fts.counters.scanned_records, 0);
    let stream_request = selected_fts(None, "Needle", false);
    let mut streamed = Vec::new();
    let stream = repo
        .search_stream(
            &stream_request,
            crate::streaming::StreamOptions {
                mode: crate::streaming::StreamMode::PageReplacements,
                ..Default::default()
            },
            &crate::streaming::CancellationToken::new(),
            &mut |event| {
                if let crate::streaming::StreamEvent::PageReplacement { rows, .. } = event {
                    streamed.extend(rows.iter().map(|row| row.record_id.clone()));
                }
                Ok(crate::streaming::StreamControl::Continue)
            },
        )
        .unwrap();
    let crate::streaming::StreamOutcome::Complete(summary) = stream else {
        panic!("selected relation stream did not complete");
    };
    assert_eq!(summary.count, Some(1));
    assert!(summary.counters.files_opened <= 1);
    assert!(summary.counters.relation_target_reads <= 1);
    assert_eq!(streamed, ["S0"]);
    repo.configure_verified_cache(0).unwrap();
    let mut uncached = Vec::new();
    let outcome = repo
        .search_stream(
            &stream_request,
            crate::streaming::StreamOptions {
                mode: crate::streaming::StreamMode::PageReplacements,
                ..Default::default()
            },
            &crate::streaming::CancellationToken::new(),
            &mut |event| {
                if let crate::streaming::StreamEvent::PageReplacement { rows, .. } = event {
                    uncached.extend(rows.iter().map(|row| row.record_id.clone()));
                }
                Ok(crate::streaming::StreamControl::Continue)
            },
        )
        .unwrap();
    let crate::streaming::StreamOutcome::Complete(summary) = outcome else {
        panic!("uncached relation stream did not complete");
    };
    assert_eq!(summary.count, Some(1));
    assert_eq!(uncached, ["S0"]);
    let filtered = selected_fts(Some("@record.id = 'S1'"), "Needle", false);
    assert!(ids(&repo, &filtered).is_empty());
    let filtered = selected_fts(Some("@record.id = 'S1'"), "Other", false);
    assert_eq!(ids(&repo, &filtered), ["S1"]);
    let filtered = repo.search(&filtered).unwrap();
    assert_eq!(filtered.count, Some(1));
    assert!(
        filtered.hits[0]
            .matched_fields
            .iter()
            .any(|field| field == "entry.title")
    );
    assert_eq!(filtered.counters.scanned_records, 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn relation_target_match_in_second_segment_is_not_redirected_to_first() {
    let root = std::env::temp_dir().join(format!(
        "objekte-semijoin-target-local-slots-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&root);
    for name in ["sources", "targets"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    for number in 0..513 {
        let (id, title) = if number == 512 {
            ("T512".to_owned(), "Needle")
        } else {
            (format!("T{number:03}"), "Other")
        };
        std::fs::write(
            root.join(format!("targets/{number:03}.json")),
            format!(r#"{{"id":"{id}","title":"{title}","related_entry":"T000"}}"#),
        )
        .unwrap();
    }
    for number in 0..600 {
        let (id, target) = if number == 0 {
            ("S0".to_owned(), "T512")
        } else {
            (format!("S{number:03}"), "T000")
        };
        std::fs::write(
            root.join(format!("sources/{number:03}.json")),
            format!(r#"{{"id":"{id}","entry":"{target}","title":"local"}}"#),
        )
        .unwrap();
    }

    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions(),
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(1).unwrap(),
                max_files: std::num::NonZeroUsize::new(1).unwrap(),
            },
            ..Default::default()
        },
        crate::source::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let target_index = repo
        .collections()
        .iter()
        .position(|collection| collection.definition.name == "targets")
        .unwrap();
    assert_eq!(repo.segments[target_index].len(), 2);
    assert_eq!(
        repo.segments[target_index][0].index.records[0]
            .id
            .0
            .as_ref(),
        "T000"
    );
    assert_eq!(repo.segments[target_index][0].index.records[0].slot.0, 0);
    assert_eq!(
        repo.segments[target_index][1].index.records[0]
            .id
            .0
            .as_ref(),
        "T512"
    );
    assert_eq!(repo.segments[target_index][1].index.records[0].slot.0, 0);

    let request = query("entry.title = 'Needle'");
    assert_eq!(ids(&repo, &request), ["S0"]);
    assert_eq!(repo.search(&request).unwrap().count, Some(1));
    assert!(
        repo.explain_analyze(&request).unwrap().1[0]
            .terms
            .iter()
            .any(|choice| {
                choice.selected.kind == crate::executor::AccessPathKind::RelationTargetFirst
            })
    );
    let source_index = repo
        .collections()
        .iter()
        .position(|collection| collection.definition.name == "sources")
        .unwrap();
    let bound = crate::relation_query::bind_filter_with_relations(
        &repo,
        source_index,
        request.filter.as_ref().unwrap(),
        &mut HashMap::new(),
    )
    .unwrap();
    let BoundExpr::Term(term) = bound else {
        panic!("expected structured relation term");
    };
    let view = crate::relation_query::MergedRelationView::new(
        &repo,
        &repo.active,
        &repo.segments,
        &repo.catalog,
    )
    .unwrap();
    let mut input = crate::index::segment::SegmentRecordSet::default();
    for published in repo.segments[source_index].iter() {
        for slot in published.active.bitmap().iter() {
            input.insert(crate::index::segment::SegmentRecordAddress {
                collection: published.index.collection,
                segment: published.id,
                slot: crate::model::RecordSlot(slot),
            });
        }
    }
    for strategy in [Strategy::TargetFirst, Strategy::SourceRestricted] {
        let (matches, work) =
            super::qualified_candidates_with_strategy(&view, &term, &input, strategy, None)
                .unwrap();
        assert_eq!(matches.len(), 1, "{strategy:?}");
        assert!(matches.iter().any(|address| {
            let segment = repo.segments[address.collection.0 as usize]
                .iter()
                .find(|segment| segment.id == address.segment)
                .unwrap();
            segment.index.record(address.slot).unwrap().id.0.as_ref() == "S0"
        }));
        assert!(work.target_candidates >= 1);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn relation_fts_counts_distinct_local_slots_in_three_segments_once() {
    let root = std::env::temp_dir().join(format!(
        "objekte-semijoin-three-segments-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&root);
    for name in ["sources", "targets"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    std::fs::write(
        root.join("targets/T0.json"),
        r#"{"id":"T0","title":"Needle","related_entry":"T0"}"#,
    )
    .unwrap();
    let matching = std::collections::HashMap::from([
        (0usize, "S0"),
        (1, "S1"),
        (512, "S2"),
        (513, "S3"),
        (1_024, "S4"),
    ]);
    for number in 0..1_025usize {
        let id = matching
            .get(&number)
            .map_or_else(|| format!("F{number:04}"), |id| (*id).to_owned());
        let entry = if matching.contains_key(&number) {
            "T0"
        } else {
            "missing"
        };
        std::fs::write(
            root.join(format!("sources/{number:04}.json")),
            format!(r#"{{"id":"{id}","entry":"{entry}","title":"local"}}"#),
        )
        .unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions(),
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(1).unwrap(),
                max_files: std::num::NonZeroUsize::new(1).unwrap(),
            },
            ..Default::default()
        },
        crate::source::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let source_index = repo
        .collections()
        .iter()
        .position(|collection| collection.definition.name == "sources")
        .unwrap();
    let source_segments = &repo.segments[source_index];
    assert_eq!(source_segments.len(), 3);
    assert_eq!(
        source_segments
            .iter()
            .map(|segment| segment.index.records.len())
            .collect::<Vec<_>>(),
        [512, 512, 1]
    );
    for segment in &source_segments[..2] {
        assert_eq!(segment.index.records[0].slot.0, 0);
        assert_eq!(segment.index.records[1].slot.0, 1);
    }
    assert_eq!(
        source_segments
            .iter()
            .map(|segment| segment.id)
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    for request in [
        query("entry.title = 'Needle'"),
        selected_fts(None, "Needle", false),
    ] {
        assert_eq!(ids(&repo, &request), ["S0", "S1", "S2", "S3", "S4"]);
        let result = repo.search(&request).unwrap();
        assert_eq!(result.count, Some(5));
        assert_eq!(result.counters.scanned_records, 0);
        assert!(result.hits.iter().all(|hit| {
            hit.record_id != "S0"
                || hit
                    .matched_fields
                    .iter()
                    .any(|field| field == "entry.title")
        }));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn related_fts_reads_current_target_revisions_across_file_lifecycle_and_compaction() {
    let root = std::env::temp_dir().join(format!(
        "objekte-related-fts-target-lifecycle-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&root);
    for name in ["sources", "targets"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    std::fs::write(
        root.join("targets/T0.json"),
        r#"{"id":"T0","title":"Needle","related_entry":"T0"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("sources/S0.json"),
        r#"{"id":"S0","entry":"T0","title":"local"}"#,
    )
    .unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions(),
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let original_inventory = index.view().unwrap().repository().files.clone();
    let assert_ids = |expected: &[&str], literal: &str, filter: Option<&str>| {
        let request = selected_fts(filter, literal, false);
        let view = index.view().unwrap();
        let result = view.search(&request).unwrap();
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(result.count, Some(expected.len()));
        assert_eq!(result.hits, view.search_reference(&request).unwrap().hits);
        assert_eq!(result.counters.scanned_records, 0);
        if !expected.is_empty() {
            assert!(result.counters.relation_target_reads > 0);
        }
        assert!(result.hits.iter().all(|hit| {
            hit.matched_fields
                .iter()
                .any(|field| field == "entry.title")
        }));
    };
    assert_ids(&["S0"], "Needle", None);

    let target = root.join("targets/T2.json");
    std::fs::write(
        &target,
        r#"{"id":"T2","title":"Needle","related_entry":"T2"}"#,
    )
    .unwrap();
    let mut target_handle = index.add_file(&target).unwrap().file;
    assert!(
        original_inventory
            .files
            .get(target_handle.file_id.0 as usize)
            .is_none(),
        "the new target must not exist in the discovery inventory"
    );
    let source = root.join("sources/S2.json");
    std::fs::write(&source, r#"{"id":"S2","entry":"T2","title":"local"}"#).unwrap();
    index.add_file(&source).unwrap();
    assert_ids(&["S0", "S2"], "Needle", None);

    std::fs::write(
        &target,
        r#"{"id":"T2","title":"Other","related_entry":"T2"}"#,
    )
    .unwrap();
    target_handle = index.update_file(target_handle, &target).unwrap().file;
    assert_ids(&["S0"], "Needle", None);
    assert_ids(&[], "CompletelyDifferent", None);

    std::fs::write(
        &target,
        r#"{"id":"T2","title":"CompletelyDifferent","related_entry":"T2"}"#,
    )
    .unwrap();
    target_handle = index.update_file(target_handle, &target).unwrap().file;
    assert_ids(&[], "Needle", Some("@record.id = 'S2'"));
    assert_ids(&["S2"], "CompletelyDifferent", None);

    let moved = root.join("targets/moved-T2.json");
    std::fs::rename(&target, &moved).unwrap();
    target_handle = index.update_file(target_handle, &moved).unwrap().file;
    assert!(!target.exists());
    assert_ids(&["S2"], "CompletelyDifferent", Some("@record.id = 'S2'"));

    index.start_compaction().unwrap().wait().unwrap();
    assert_ids(&["S2"], "CompletelyDifferent", Some("@record.id = 'S2'"));

    index.delete_file(target_handle).unwrap();
    assert_ids(&[], "CompletelyDifferent", Some("@record.id = 'S2'"));
    assert_ids(&["S0"], "Needle", None);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

fn next_random(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1);
    *state >> 32
}

fn seeded_array_fixture(
    alphabet: BigramAlphabet,
    seed: u64,
) -> (std::path::PathBuf, Repository, Vec<String>, Vec<String>) {
    let root = std::env::temp_dir().join(format!(
        "objekte-semijoin-seeded-{}-{seed}-{}",
        std::process::id(),
        alphabet as u8,
    ));
    for name in ["sources", "targets"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    let mut random = seed;
    let mut needle_targets = HashSet::new();
    for index in 0..12 {
        let title = if next_random(&mut random).is_multiple_of(3) {
            needle_targets.insert(format!("T{index:02}"));
            "Needle"
        } else {
            "Other"
        };
        std::fs::write(
            root.join(format!("targets/T{index:02}.json")),
            serde_json::json!({"id": format!("T{index:02}"), "title": title}).to_string(),
        )
        .unwrap();
    }
    let mut expected_equal = Vec::new();
    let mut expected_not_equal = Vec::new();
    for index in 0..32 {
        let id = format!("S{index:02}");
        let value = if index % 11 == 0 {
            serde_json::json!({"id": id})
        } else if index % 11 == 1 {
            serde_json::json!({"id": id, "links": null})
        } else {
            let count = if index == 3 {
                100
            } else {
                (next_random(&mut random) % 5) as usize
            };
            let mut links = Vec::with_capacity(count);
            for _ in 0..count {
                let target = if next_random(&mut random).is_multiple_of(9) {
                    "missing".to_owned()
                } else {
                    format!("T{:02}", next_random(&mut random) % 12)
                };
                links.push(target);
            }
            if links.iter().any(|target| needle_targets.contains(target)) {
                expected_equal.push(id.clone());
            }
            if links
                .iter()
                .any(|target| target != "missing" && !needle_targets.contains(target))
            {
                expected_not_equal.push(id.clone());
            }
            serde_json::json!({"id": id, "links": links})
        };
        std::fs::write(
            root.join(format!("sources/S{index:02}.json")),
            value.to_string(),
        )
        .unwrap();
    }
    let mut targets = Schema::default();
    field(&mut targets, "id", LogicalType::Id);
    field(&mut targets, "title", LogicalType::String);
    let mut sources = Schema::default();
    field(&mut sources, "id", LogicalType::Id);
    sources
        .register_field(FieldDefinition {
            path: vec!["links".into()],
            kind: LogicalType::Array(Box::new(LogicalType::Id)),
            required: false,
            nullable: true,
            indexed: true,
            multi: true,
            element_nullable: vec![false],
        })
        .unwrap();
    sources
        .register_relation(RelationDefinition {
            path: vec!["links".into()],
            target_collection: "targets".into(),
            cardinality: RelationCardinality::Many,
            reverse_alias: Some("sources".into()),
        })
        .unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![
            definition("sources", sources),
            definition("targets", targets),
        ],
        RepositoryOptions {
            bigram_alphabet: alphabet,
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    (root, repo, expected_equal, expected_not_equal)
}

#[test]
fn both_physical_strategies_keep_independent_witnesses_and_reuse_targets() {
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let (root, repo) = fixture(alphabet);
        let request = query("entry.title ?~ 'Hymn'");
        let source_index = repo
            .collections
            .iter()
            .position(|item| item.definition.name == "sources")
            .unwrap();
        let bound = crate::relation_query::bind_filter_with_relations(
            &repo,
            source_index,
            request.filter.as_ref().unwrap(),
            &mut HashMap::new(),
        )
        .unwrap();
        let BoundExpr::Term(term) = bound else {
            panic!("expected term")
        };
        let mut same_semantics = term.clone();
        same_semantics.span = (0, 0);
        assert!(crate::reference::same_bound_term(&term, &same_semantics));
        let mut different_literal = term.clone();
        different_literal.literal = Value::String("Other".into());
        assert!(!crate::reference::same_bound_term(
            &term,
            &different_literal
        ));
        let mut different_target = term.clone();
        different_target.related.as_mut().unwrap().hops[0].to.0 += 1;
        assert!(!crate::reference::same_bound_term(&term, &different_target));
        let source = &repo.collections[source_index];
        let all = source.logical_records.clone();
        let (target_first, target_work) =
            candidates_with_strategy(&repo, &term, &all, Strategy::TargetFirst).unwrap();
        let (source_first, source_work) =
            candidates_with_strategy(&repo, &term, &all, Strategy::SourceRestricted).unwrap();
        assert_eq!(target_first, source_first);
        assert_eq!(target_first.len(), 40);
        assert!(target_work.target_reads <= 1);
        assert!(source_work.target_reads <= 25);
        let expected = (0..40).map(|n| format!("S{n:02}")).collect::<Vec<_>>();
        assert_eq!(ids(&repo, &request), expected);
        let actual = repo.search(&request).unwrap();
        assert!(actual.counters.relation_target_reads <= 1);
        assert_eq!(actual.counters.relation_target_candidates, 1);
        assert_eq!(actual.counters.relation_reverse_postings_visited, 1);
        assert_eq!(actual.counters.relation_reverse_members_visited, 40);
        assert_eq!(actual.counters.scanned_records, 0);
        let trace = repo.explain_analyze(&request).unwrap().1;
        assert!(
            trace[0].terms.iter().any(|choice| choice.selected.kind
                == crate::executor::AccessPathKind::RelationTargetFirst)
        );

        let selective = query("title = 'rare' && entry.title ?~ 'H'");
        assert_eq!(ids(&repo, &selective), ["S00"]);
        let trace = repo.explain_analyze(&selective).unwrap().1;
        assert!(trace[0].terms.iter().any(|choice| choice.selected.kind
            == crate::executor::AccessPathKind::RelationSourceRestricted));
        let selective_work = repo.search(&selective).unwrap().counters;
        assert!(selective_work.relation_target_reads <= 1);
        assert!(selective_work.relation_edge_visits <= 1);
        let selective_fts = selected_fts(Some("title = 'rare'"), "H", false);
        assert_eq!(ids(&repo, &selective_fts), ["S00"]);
        let selective_fts = repo.search(&selective_fts).unwrap();
        assert_eq!(selective_fts.counters.relation_edge_visits, 1);
        assert_eq!(selective_fts.counters.scanned_records, 0);

        let multi = query("entry.related_entry.title ?= 'Festival'");
        assert_eq!(
            ids(&repo, &multi),
            (0..59).map(|n| format!("S{n:02}")).collect::<Vec<_>>()
        );
        assert!(
            repo.explain_analyze(&multi).unwrap().1[0]
                .terms
                .iter()
                .any(|choice| matches!(
                    choice.selected.kind,
                    crate::executor::AccessPathKind::RelationTargetFirst
                        | crate::executor::AccessPathKind::RelationSourceRestricted
                ))
        );

        let negative = query("!(entry.title ?~ 'Hymn')");
        let negative_ids = ids(&repo, &negative);
        assert!(!negative_ids.contains(&"S59".to_owned()));
        assert!(
            repo.explain(&negative).unwrap()[0]
                .terms
                .iter()
                .all(|choice| !matches!(
                    choice.selected.kind,
                    crate::executor::AccessPathKind::RelationTargetFirst
                        | crate::executor::AccessPathKind::RelationSourceRestricted
                ))
        );
        let universal = query("entry.title = 'Hymn Parade'");
        assert_eq!(ids(&repo, &universal), expected);
        assert!(
            repo.explain_analyze(&universal).unwrap().1[0]
                .terms
                .iter()
                .any(|choice| matches!(
                    choice.selected.kind,
                    crate::executor::AccessPathKind::RelationTargetFirst
                        | crate::executor::AccessPathKind::RelationSourceRestricted
                ))
        );
        let fallback_or = query("@file.status = 1 || entry.title ?~ 'Hymn'");
        let fallback_ids = ids(&repo, &fallback_or);
        assert_eq!(fallback_ids.len(), 41);
        assert!(
            repo.search(&fallback_or)
                .unwrap()
                .hits
                .iter()
                .any(|hit| hit.record_class == crate::inventory::RecordClass::RawFallback)
        );

        for (literal, regex) in [("Hymn", false), ("^Hymn.*$", true)] {
            let request = selected_fts(None, literal, regex);
            assert_eq!(ids(&repo, &request), expected);
            let result = repo.search(&request).unwrap();
            assert!(result.counters.relation_target_reads <= 1);
            assert_eq!(result.counters.relation_target_candidates, 1);
            assert_eq!(result.counters.relation_reverse_postings_visited, 1);
            assert_eq!(result.counters.relation_reverse_members_visited, 40);
            assert_eq!(result.counters.scanned_records, 0);
            assert!(
                repo.explain_analyze(&request).unwrap().1[0]
                    .fts_strategy
                    .starts_with("relation target-first")
            );
        }
        let selected = selected_fts(Some("title = 'rare'"), "H", false);
        assert_eq!(ids(&repo, &selected), ["S00"]);
        let trace = repo.explain_analyze(&selected).unwrap().1;
        assert!(
            trace[0]
                .fts_strategy
                .starts_with("relation source-restricted")
        );
        let selected_regex = selected_fts(Some("title = 'rare'"), "H|Other", true);
        assert_eq!(ids(&repo, &selected_regex), ["S00"]);
        let regex_work = repo.search(&selected_regex).unwrap().counters;
        assert_eq!(regex_work.relation_edge_visits, 1);
        assert_eq!(regex_work.relation_target_candidates, 1);
        assert!(
            repo.explain_analyze(&selected_regex).unwrap().1[0]
                .fts_strategy
                .starts_with("relation source-restricted")
        );
        let mixed = selected_fts_fields(
            None,
            "Hymn",
            false,
            vec!["title".into(), "entry.title".into()],
        );
        assert_eq!(ids(&repo, &mixed), expected);
        assert!(
            !repo.explain(&mixed).unwrap()[0]
                .fts_strategy
                .starts_with("relation")
        );
        let snapshot_options = RepositoryOptions {
            bigram_alphabet: alphabet,
            verified_cache_bytes: 0,
            ..Default::default()
        };
        let fresh = crate::lifecycle::Index::build_repository_for_test(
            &root,
            definitions(),
            snapshot_options,
            crate::source::SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        let restored = crate::lifecycle::Index::build_repository_for_test(
            &root,
            definitions(),
            snapshot_options,
            crate::source::SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        for request in [
            query("entry.title ?~ 'Hymn'"),
            selected_fts(None, "^Hymn.*$", true),
        ] {
            let before = fresh.search(&request).unwrap();
            let after = restored.search(&request).unwrap();
            assert_eq!(before.hits, after.hits);
            assert_eq!(before.count, after.count);
            assert_eq!(
                fresh.explain(&request).unwrap()[0].candidates,
                restored.explain(&request).unwrap()[0].candidates
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn seeded_array_semijoins_match_an_index_free_expected_enumeration() {
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        for seed in [0x317_u64, 0x9ab, 0x15cc, 0x2801] {
            let (root, repo, equal, not_equal) = seeded_array_fixture(alphabet, seed);
            let source_index = repo
                .collections
                .iter()
                .position(|item| item.definition.name == "sources")
                .unwrap();
            let source = &repo.collections[source_index];
            for (filter, expected) in [
                ("links.title ?= 'Needle'", &equal),
                ("links.title ?!= 'Needle'", &not_equal),
            ] {
                let request = query(filter);
                assert_eq!(ids(&repo, &request), *expected, "seed={seed} {filter}");
                let bound = crate::relation_query::bind_filter_with_relations(
                    &repo,
                    source_index,
                    request.filter.as_ref().unwrap(),
                    &mut HashMap::new(),
                )
                .unwrap();
                let BoundExpr::Term(term) = bound else {
                    panic!("expected one term")
                };
                let expected_slots = source
                    .generation
                    .records
                    .iter()
                    .filter(|record| {
                        expected
                            .iter()
                            .any(|id| id.as_str() == record.id.0.as_ref())
                    })
                    .map(|record| record.slot.0)
                    .collect::<roaring::RoaringBitmap>();
                for strategy in [Strategy::TargetFirst, Strategy::SourceRestricted] {
                    let (actual, _) =
                        candidates_with_strategy(&repo, &term, &source.logical_records, strategy)
                            .unwrap();
                    assert_eq!(actual, expected_slots, "seed={seed} {filter} {strategy:?}");
                }
            }
            let both_expected = equal
                .iter()
                .filter(|id| not_equal.contains(id))
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(
                ids(
                    &repo,
                    &query("links.title ?= 'Needle' && links.title ?= 'Other'")
                ),
                both_expected,
                "seed={seed} independent existential witnesses",
            );
            let universal = query("links.title = 'Needle'");
            let _ = ids(&repo, &universal);
            assert!(
                repo.explain(&universal).unwrap()[0]
                    .terms
                    .iter()
                    .all(|choice| !matches!(
                        choice.selected.kind,
                        crate::executor::AccessPathKind::RelationTargetFirst
                            | crate::executor::AccessPathKind::RelationSourceRestricted
                    ))
            );
            let positional = query("links[0].title ?= 'Needle'");
            let _ = ids(&repo, &positional);
            assert!(
                repo.explain(&positional).unwrap()[0]
                    .terms
                    .iter()
                    .all(|choice| !matches!(
                        choice.selected.kind,
                        crate::executor::AccessPathKind::RelationTargetFirst
                            | crate::executor::AccessPathKind::RelationSourceRestricted
                    ))
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
