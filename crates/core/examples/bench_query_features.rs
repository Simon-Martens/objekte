//! Small fixed feature workload. Build once per profile; query samples exclude
//! fixture creation, index construction, reference checks, and EXPLAIN.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use objekte_search_core::model::{FieldPath, LogicalType, Value};
use objekte_search_core::query::{CaseSensitivity, FtsMode, Query, RegexOptions};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::relation::{RelationCardinality, RelationDefinition};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::{BigramAlphabet, ReturnMode, WorkloadCase};
use serde_json::json;


struct FeatureCase {
    id: &'static str,
    collection: &'static str,
    filter: Option<&'static str>,
    fts: Option<&'static str>,
    regex: bool,
    fts_fields: &'static [&'static str],
    parameters: HashMap<String, Value>,
    expected_page: &'static [&'static str],
    expected_count: usize,
    ordered: bool,
    offset: usize,
    limit: usize,
}


impl FeatureCase {
    fn query(&self) -> Result<Query, String> {
        Query::from_workload(&WorkloadCase {
            id: self.id.into(),
            collection: self.collection.into(),
            fts: self.fts.map(str::to_owned),
            fts_case_sensitivity: CaseSensitivity::Sensitive,
            fts_mode: if self.regex {
                FtsMode::Regex(RegexOptions::default())
            } else {
                FtsMode::Literal
            },
            filter: self.filter.map(str::to_owned),
            fts_fields: self
                .fts_fields
                .iter()
                .map(|field| (*field).into())
                .collect(),
            sort: vec![],
            limit: self.limit,
            offset: self.offset,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        })
    }
}


fn add_field(schema: &mut Schema, name: &str, kind: LogicalType, multi: bool) {
    schema
        .register_field(FieldDefinition {
            path: vec![name.into()],
            kind,
            required: name == "id" || name == "title",
            nullable: name != "id" && name != "title",
            indexed: true,
            multi,
            element_nullable: vec![],
        })
        .unwrap();
}

fn definitions() -> Vec<CollectionDefinition> {
    let mut entries = Schema::default();
    add_field(&mut entries, "id", LogicalType::Id, false);
    add_field(&mut entries, "title", LogicalType::String, false);
    add_field(&mut entries, "issued", LogicalType::Date, false);
    add_field(&mut entries, "declared_size", LogicalType::Number, false);
    let mut mentions = Schema::default();
    add_field(&mut mentions, "id", LogicalType::Id, false);
    add_field(&mut mentions, "title", LogicalType::String, false);
    add_field(&mut mentions, "local_title", LogicalType::String, false);
    add_field(&mut mentions, "entry", LogicalType::Id, false);
    add_field(
        &mut mentions,
        "contributors",
        LogicalType::Array(Box::new(LogicalType::Id)),
        true,
    );
    for (path, cardinality, alias) in [
        ("entry", RelationCardinality::One, "mentions"),
        ("contributors", RelationCardinality::Many, "contributions"),
    ] {
        mentions
            .register_relation(RelationDefinition {
                path: vec![path.into()],
                target_collection: "entries".into(),
                cardinality,
                reverse_alias: Some(alias.into()),
            })
            .unwrap();
    }
    [("entries", entries), ("mentions", mentions)]
        .into_iter()
        .map(|(name, schema)| CollectionDefinition {
            name: name.into(),
            source_path: name.into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
            discovery: Default::default(),
            schema,
            sort_fields: if name == "entries" {
                vec![
                    FieldPath::parse("title").unwrap(),
                    FieldPath::parse("issued").unwrap(),
                ]
            } else {
                vec![FieldPath::parse("title").unwrap()]
            },
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        })
        .collect()
}

fn fixture(root: &Path) -> Result<(), String> {
    for collection in ["entries", "mentions"] {
        std::fs::create_dir_all(root.join(collection)).map_err(|error| error.to_string())?;
    }
    for (path, body) in [
        (
            "entries/a.json",
            r#"{"id":"A","title":"Hymn","issued":"2024-02-29","declared_size":1536}"#,
        ),
        (
            "entries/b.json",
            r#"{"id":"B","title":"Festival","issued":"2024-03-01","declared_size":2000}"#,
        ),
        (
            "entries/c.json",
            r#"{"id":"C","title":"Aria","issued":"2024-02-01","declared_size":1000}"#,
        ),
        (
            "mentions/m1.json",
            r#"{"id":"m1","entry":"A","contributors":["A","B"],"title":"Festival","local_title":"Hymn"}"#,
        ),
        (
            "mentions/m2.json",
            r#"{"id":"m2","entry":"B","contributors":["B"],"title":"Hymn","local_title":"Festival"}"#,
        ),
        (
            "mentions/m3.json",
            r#"{"id":"m3","entry":"Z","contributors":["Z"],"title":"Dangling"}"#,
        ),
        (
            "mentions/m4.json",
            r#"{"id":"m4","entry":"C","contributors":["A","C"],"title":"Other"}"#,
        ),
        (
            "mentions/m5.json",
            r#"{"id":"m5","entry":"A","contributors":[],"title":"Extra"}"#,
        ),
        (
            "mentions/m6.json",
            r#"{"id":"m6","entry":null,"contributors":[],"title":"Empty"}"#,
        ),
    ] {
        std::fs::write(root.join(path), body).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn cases() -> Vec<FeatureCase> {
    let mut cases = Vec::new();
    let mut add = |id,
                   collection,
                   filter,
                   fts,
                   regex,
                   fts_fields,
                   parameters,
                   expected_page,
                   expected_count,
                   ordered,
                   offset,
                   limit| {
        cases.push(FeatureCase {
            id,
            collection,
            filter,
            fts,
            regex,
            fts_fields,
            parameters,
            expected_page,
            expected_count,
            ordered,
            offset,
            limit,
        });
    };
    let id = |value: &str| HashMap::from([("target".into(), Value::Id(value.into()))]);
    add(
        "parameter-a",
        "mentions",
        Some("entry = $target"),
        None,
        false,
        &[],
        id("A"),
        &["m1", "m5"],
        2,
        false,
        0,
        20,
    );
    add(
        "parameter-b",
        "mentions",
        Some("entry = $target"),
        None,
        false,
        &[],
        id("B"),
        &["m2"],
        1,
        false,
        0,
        20,
    );
    add(
        "position",
        "mentions",
        Some("contributors[0].title = 'Hymn'"),
        None,
        false,
        &[],
        HashMap::new(),
        &["m1", "m4"],
        2,
        false,
        0,
        20,
    );
    add(
        "field-comparison",
        "mentions",
        Some("local_title = entry.title"),
        None,
        false,
        &[],
        HashMap::new(),
        &["m1", "m2"],
        2,
        false,
        0,
        20,
    );
    add(
        "sort-only",
        "mentions",
        Some("SORT BY title DESC"),
        None,
        false,
        &[],
        HashMap::new(),
        &["m4", "m2", "m1", "m5", "m6", "m3"],
        6,
        true,
        0,
        20,
    );
    add(
        "sort-deep-page",
        "mentions",
        Some("SORT BY title DESC"),
        None,
        false,
        &[],
        HashMap::new(),
        &["m6", "m3"],
        6,
        true,
        4,
        2,
    );
    add(
        "sort-parameter",
        "mentions",
        Some("entry = $target SORT BY title DESC"),
        None,
        false,
        &[],
        id("A"),
        &["m1", "m5"],
        2,
        true,
        0,
        20,
    );
    add(
        "sort-multi-key",
        "entries",
        Some("SORT BY issued ASC, title DESC"),
        None,
        false,
        &[],
        HashMap::new(),
        &["C", "A", "B"],
        3,
        true,
        0,
        20,
    );
    add(
        "temporal",
        "entries",
        Some("issued = 2024-02"),
        None,
        false,
        &[],
        HashMap::new(),
        &["A", "C"],
        2,
        false,
        0,
        20,
    );
    add(
        "temporal-sort",
        "entries",
        Some("issued = 2024-02 SORT BY title ASC"),
        None,
        false,
        &[],
        HashMap::new(),
        &["C", "A"],
        2,
        true,
        0,
        20,
    );
    add(
        "byte-size",
        "entries",
        Some("declared_size = 1.5KiB"),
        None,
        false,
        &[],
        HashMap::new(),
        &["A"],
        1,
        false,
        0,
        20,
    );
    add(
        "regex-useful",
        "entries",
        None,
        Some("^Hy.*n$"),
        true,
        &["title"],
        HashMap::new(),
        &["A"],
        1,
        false,
        0,
        20,
    );
    add(
        "regex-unrestricted",
        "entries",
        None,
        Some("Hymn|.*"),
        true,
        &["title"],
        HashMap::new(),
        &["A", "B", "C"],
        3,
        false,
        0,
        20,
    );
    add(
        "relation-target-first",
        "mentions",
        Some("entry.title = 'Hymn'"),
        None,
        false,
        &[],
        HashMap::new(),
        &["m1", "m5"],
        2,
        false,
        0,
        20,
    );
    add(
        "relation-source-restricted",
        "mentions",
        Some("title = 'Festival' && entry.title = 'Hymn'"),
        None,
        false,
        &[],
        HashMap::new(),
        &["m1"],
        1,
        false,
        0,
        20,
    );
    add(
        "relation-any",
        "mentions",
        Some("contributors.title ?= 'Hymn'"),
        None,
        false,
        &[],
        HashMap::new(),
        &["m1", "m4"],
        2,
        false,
        0,
        20,
    );
    add(
        "selected-fts-position",
        "mentions",
        None,
        Some("Hymn"),
        false,
        &["contributors[0].title"],
        HashMap::new(),
        &["m1", "m4"],
        2,
        false,
        0,
        20,
    );
    add(
        "relation-dangling-not",
        "mentions",
        Some("!(entry.title = 'Hymn')"),
        None,
        false,
        &[],
        HashMap::new(),
        &["m2", "m4", "m6"],
        3,
        false,
        0,
        20,
    );
    add(
        "backlinks",
        "entries",
        Some("@backlinks.mentions.title ?= 'Festival'"),
        None,
        false,
        &[],
        HashMap::new(),
        &["A"],
        1,
        false,
        0,
        20,
    );
    let mut mixed = id("A");
    mixed.insert("pattern".into(), Value::String("^Hy.*n$".into()));
    add(
        "regex-parameter-relation",
        "mentions",
        Some("entry = $target && title = 'Festival'"),
        Some("$pattern"),
        true,
        &["entry.title"],
        mixed,
        &["m1"],
        1,
        false,
        0,
        20,
    );
    let mut sorted_mixed = id("A");
    sorted_mixed.insert("pattern".into(), Value::String("^Hy.*n$".into()));
    add(
        "regex-parameter-relation-sort",
        "mentions",
        Some("entry = $target SORT BY title DESC"),
        Some("$pattern"),
        true,
        &["entry.title"],
        sorted_mixed,
        &["m1", "m5"],
        2,
        true,
        0,
        20,
    );
    cases
}

fn main() -> Result<(), String> {
    let mut root = None;
    let mut alphabet = BigramAlphabet::Ascii;
    let mut cache_bytes = 0;
    let mut repetitions = 5;
    let mut check_only = false;
    let mut revision = String::from("unknown");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(args.next().ok_or("--root needs a path")?)),
            "--alphabet" => {
                alphabet = match args.next().as_deref() {
                    Some("ascii") => BigramAlphabet::Ascii,
                    Some("unicode") => BigramAlphabet::Unicode,
                    _ => return Err("--alphabet needs ascii or unicode".into()),
                }
            }
            "--cache" => {
                cache_bytes = match args.next().as_deref() {
                    Some("off") => 0,
                    Some("warm") => 64 * 1024 * 1024,
                    _ => return Err("--cache needs off or warm".into()),
                }
            }
            "--repetitions" => {
                repetitions = args
                    .next()
                    .ok_or("--repetitions needs a count")?
                    .parse()
                    .map_err(|error: std::num::ParseIntError| error.to_string())?
            }
            "--revision" => revision = args.next().ok_or("--revision needs a SHA")?,
            "--check-only" => check_only = true,
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    let root = root.ok_or("--root is required")?;
    fixture(&root)?;
    let build_started = Instant::now();
    let repo = Repository::open_with_options(
        &root,
        definitions(),
        RepositoryOptions {
            bigram_alphabet: alphabet,
            verified_cache_bytes: cache_bytes,
            snapshot_mode: SnapshotMode::Disabled,
            query_workers: 1,
            ..Default::default()
        },
    )?;
    let build_ns = build_started.elapsed().as_nanos() as u64;
    let startup = repo.startup();
    let relations = repo
        .collections()
        .iter()
        .flat_map(|collection| &collection.relations)
        .collect::<Vec<_>>();
    if !check_only {
        println!(
            "{}",
            json!({"kind":"build","revision":revision,"alphabet":alphabet,
            "cache_bytes":cache_bytes,"wall_ns":build_ns,"build_ns":startup.build_ns,
            "records":repo.collections().iter().map(|collection| collection.generation.records.len()).sum::<usize>(),
            "relation_count":relations.len(),
            "relation_edges":relations.iter().map(|relation| relation.edge_count()).sum::<usize>(),
            "relation_heap_bytes":relations.iter().map(|relation| relation.estimated_heap_bytes()).sum::<usize>()})
        );
    }
    for case in cases() {
        let query = case.query()?;
        let reference = repo.search_reference_with_parameters(&query, &case.parameters)?;
        let checked = repo.search_with_parameters(&query, &case.parameters)?;
        if checked.hits != reference.hits || checked.count != reference.count {
            return Err(format!("{}: optimized/reference mismatch", case.id));
        }
        let expected = case
            .expected_page
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        let normalize = |result: &objekte_search_core::repository::SearchResult| {
            let mut ids = result
                .hits
                .iter()
                .map(|hit| hit.record_id.clone())
                .collect::<Vec<_>>();
            if !case.ordered {
                ids.sort();
            }
            ids
        };
        if normalize(&checked) != expected || checked.count != Some(case.expected_count) {
            return Err(format!(
                "{}: expected {:?}/{} but got {:?}/{:?}",
                case.id,
                expected,
                case.expected_count,
                normalize(&checked),
                checked.count
            ));
        }
        if check_only {
            continue;
        }
        let plan = repo.explain_with_parameters(&query, &case.parameters)?;
        println!(
            "{}",
            json!({"kind":"plan","case_id":case.id,
                "filter":case.filter,"fts":case.fts,"regex":case.regex,
                "fts_fields":case.fts_fields,"parameters":case.parameters,
                "offset":case.offset,"limit":case.limit,"expected_page":expected,
                "expected_count":case.expected_count,"plan":plan})
        );
        for repetition in 0..repetitions {
            let started = Instant::now();
            let result = repo.search_with_parameters(&query, &case.parameters)?;
            let total_ns = started.elapsed().as_nanos() as u64;
            if normalize(&result) != expected || result.count != Some(case.expected_count) {
                return Err(format!("{}: timed answer changed", case.id));
            }
            println!(
                "{}",
                json!({"kind":"sample","case_id":case.id,"alphabet":alphabet,
                "cache_bytes":cache_bytes,"repetition":repetition,"total_ns":total_ns,
                "counters":result.counters,"count":result.count,"page":normalize(&result)})
            );
        }
    }
    if check_only {
        println!("feature workload correctness: ok");
    }
    Ok(())
}
