//! Focused measurements for corrective range, page, and JSONL scan paths.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use objekte_search_core::inventory::FileStatus;
use objekte_search_core::model::{FieldPath, LogicalType, Value};
use objekte_search_core::posting::ScalarKey;
use objekte_search_core::posting_values::{PostingValueBound, PostingValuesRequest};
use objekte_search_core::query::{FtsQuery, Literal, Operator, Query, QueryOptions, parse_filter};
use objekte_search_core::reference::{
    BindingOperation, FieldDefinition, PreparedFilterMatcher, RegisteredBinding, Schema,
};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::{ReturnMode, SortField};

fn main() {
    if let Err(error) = run() {
        eprintln!("corrective benchmark error: {error}");
        std::process::exit(1);
    }
}

fn definition(
    name: &str,
    format: SourceFormat,
    schema: Schema,
    sort: bool,
) -> CollectionDefinition {
    CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: if sort {
            vec![FieldPath::parse("num").unwrap()]
        } else {
            vec![]
        },
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

fn compact(text: &str) -> String {
    text.bytes()
        .filter(|byte| !matches!(byte, b' ' | b'-' | b'_'))
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect()
}

fn compact_contains(text: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    (0..=text.len()).any(|start| {
        let mut mapped = text.as_bytes()[start..]
            .iter()
            .copied()
            .filter(|byte| !matches!(byte, b' ' | b'-' | b'_'))
            .map(|byte| byte.to_ascii_lowercase());
        needle
            .iter()
            .all(|expected| mapped.next() == Some(*expected))
    })
}


#[derive(Debug)]
struct CompactMatcher {
    needle: String,
    operator: Operator,
    callbacks: Arc<AtomicU64>,
}


impl PreparedFilterMatcher for CompactMatcher {
    fn matches(&self, value: Option<&Value>, _: Option<&[u8]>) -> Result<bool, String> {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        let Some(Value::String(text)) = value else {
            return Ok(false);
        };
        Ok(match self.operator {
            Operator::Eq => text
                .bytes()
                .filter(|byte| !matches!(byte, b' ' | b'-' | b'_'))
                .map(|byte| byte.to_ascii_lowercase())
                .eq(self.needle.bytes()),
            Operator::Contains => compact_contains(text, &self.needle),
            _ => return Err("unsupported compact operator".into()),
        })
    }

    fn posting_compatible(&self) -> bool {
        true
    }

    fn bigram_compatible(&self) -> bool {
        true
    }
}


#[derive(Debug)]
struct CompactOperation {
    preparations: Arc<AtomicU64>,
    mappings: Arc<AtomicU64>,
    callbacks: Arc<AtomicU64>,
}


impl BindingOperation for CompactOperation {
    fn accepts(&self, kind: &LogicalType, operator: Operator, _: bool) -> bool {
        *kind == LogicalType::String && matches!(operator, Operator::Eq | Operator::Contains)
    }

    fn prepare_literal(&self, literal: &Literal, _: &LogicalType) -> Result<Value, String> {
        self.preparations.fetch_add(1, Ordering::Relaxed);
        let Literal::String(text) = literal else {
            return Err("compact literal must be text".into());
        };
        Ok(Value::String(compact(
            text.strip_prefix("compact:")
                .ok_or("compact literal needs compact: prefix")?,
        )))
    }

    fn prepare_fts_literal(&self, text: &str) -> Result<String, String> {
        self.preparations.fetch_add(1, Ordering::Relaxed);
        Ok(compact(text))
    }

    fn prepare_filter_matcher(
        &self,
        literal: &Value,
        operator: Operator,
    ) -> Result<Option<Arc<dyn PreparedFilterMatcher>>, String> {
        self.preparations.fetch_add(1, Ordering::Relaxed);
        let Value::String(needle) = literal else {
            return Err("compact matcher requires text".into());
        };
        Ok(Some(Arc::new(CompactMatcher {
            needle: needle.clone(),
            operator,
            callbacks: self.callbacks.clone(),
        })))
    }

    fn map_value(&self, value: &Value) -> Option<Value> {
        let Value::String(text) = value else {
            return None;
        };
        self.mappings.fetch_add(1, Ordering::Relaxed);
        Some(Value::String(compact(text)))
    }

    fn map_text(&self, text: &str) -> String {
        self.mappings.fetch_add(1, Ordering::Relaxed);
        compact(text)
    }

    fn value_text_equivalent(&self) -> bool {
        true
    }

    fn posting(&self) -> bool {
        true
    }

    fn bigram(&self) -> bool {
        true
    }
}


fn custom_and_malformed_cases(root: &std::path::Path) -> Result<(), String> {
    let custom_root = root.join("custom-fixture");
    fs::create_dir_all(custom_root.join("data")).map_err(|e| e.to_string())?;
    let mut custom_data = String::new();
    for i in 0..1_000 {
        let title = if i % 2 == 0 { "hello world" } else { "other" };
        custom_data.push_str(&format!(r#"{{"title":"{title}"}}"#));
        custom_data.push('\n');
    }
    fs::write(custom_root.join("data/all.jsonl"), custom_data).map_err(|e| e.to_string())?;
    let preparations = Arc::new(AtomicU64::new(0));
    let mappings = Arc::new(AtomicU64::new(0));
    let callbacks = Arc::new(AtomicU64::new(0));
    let mut schema = Schema::default();
    schema.register_field(FieldDefinition {
        path: vec!["title".into()],
        kind: LogicalType::String,
        required: true,
        nullable: false,
        indexed: false,
        multi: false,
        element_nullable: vec![],
    })?;
    let binding = RegisteredBinding {
        name: "compact".into(),
        version: 1,
        operation: Arc::new(CompactOperation {
            preparations: preparations.clone(),
            mappings: mappings.clone(),
            callbacks: callbacks.clone(),
        }),
    };
    schema.register_operation(vec!["title".into()], binding.clone())?;
    schema.set_raw_binding(binding)?;
    let custom_definition = definition("data", SourceFormat::JsonLines, schema, false);
    let custom_options = RepositoryOptions {
        snapshot_mode: SnapshotMode::IfValid,
        query_workers: 4,
        ..Default::default()
    };
    for opening in 0..2 {
        let repo = Repository::open_with_options(
            &custom_root,
            vec![custom_definition.clone()],
            custom_options,
        )?;
        assert_eq!(repo.startup().restored, opening == 1);
        if opening == 0 {
            repo.save_snapshot(&objekte_search_core::storage::snapshot_path(
                &custom_root,
                custom_options.bigram_alphabet,
            ))?;
        }
        let base = Query {
            collections: vec!["data".into()],
            filter: None,
            fts: None,
            options: QueryOptions {
                sort: vec![],
                limit: 5,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        };
        for (name, filter) in [
            ("custom-posting", "title:compact = 'compact:hello world'"),
            ("custom-bigram", "title:compact ~ 'compact:hello'"),
            ("custom-scanner", "title:compact ~ 'compact:w'"),
            ("incompatible-raw", "title ~ 'hello world'"),
        ] {
            let mut query = base.clone();
            query.filter = Some(parse_filter(filter)?);
            let result = repo.search(&query)?;
            assert_eq!(result.count, Some(500), "{name}");
            measure_observed(
                &repo,
                &format!("{name}-{}", if opening == 0 { "build" } else { "restore" }),
                &query,
                11,
                Some((&callbacks, &mappings, &preparations)),
            )?;
        }
        let mut raw = base;
        raw.fts = Some(FtsQuery {
            pattern_parameter: None,
            mode: objekte_search_core::query::FtsMode::Literal,
            case_sensitivity: objekte_search_core::query::CaseSensitivity::Sensitive,
            literal: "hello world".into(),
            fields: vec![],
        });
        assert_eq!(repo.search(&raw)?.count, Some(500));
        measure(
            &repo,
            &format!(
                "custom-raw-fts-{}",
                if opening == 0 { "build" } else { "restore" }
            ),
            &raw,
            11,
        )?;
    }
    for malformed in [false, true] {
        let name = if malformed { "mixed" } else { "healthy" };
        let source_root = root.join(format!("{name}-fixture"));
        fs::create_dir_all(source_root.join("data")).map_err(|e| e.to_string())?;
        let mut data = String::new();
        for i in 0..400 {
            let padding = if i == 200 {
                "x".repeat(1_048_576)
            } else {
                "x".repeat(32)
            };
            let valid = format!(r#"{{"body":"recovery-token","padding":"{padding}"}}"#);
            if malformed && i % 40 == 0 {
                data.push_str(&valid[..valid.len() - 1]);
            } else {
                data.push_str(&valid);
            }
            data.push('\n');
        }
        fs::write(source_root.join("data/all.jsonl"), data).map_err(|e| e.to_string())?;
        let definition = definition("data", SourceFormat::JsonLines, Schema::default(), false);
        let build_options = RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            query_workers: 4,
            ..Default::default()
        };
        let mut build_samples = Vec::new();
        let mut repo = None;
        for _ in 0..11 {
            let started = Instant::now();
            let current = Repository::open_with_options(
                &source_root,
                vec![definition.clone()],
                build_options,
            )?;
            build_samples.push(started.elapsed().as_nanos() as u64);
            repo = Some(current);
        }
        build_samples.sort_unstable();
        let repo = repo.ok_or("missing build sample")?;
        assert_eq!(repo.build_failed_parses(), if malformed { 10 } else { 0 });
        println!(
            "{}",
            serde_json::json!({"case":format!("{name}-build"),"median_ns":build_samples[5],"samples_ns":build_samples,"parses":repo.build_parses(),"failed_parses":repo.build_failed_parses(),"resident_bytes":memory_bytes("VmRSS:"),"process_peak_resident_bytes":memory_bytes("VmHWM:"),"file_status":repo.files().files[0].status.code()})
        );
        if malformed {
            assert_eq!(repo.files().files[0].status, FileStatus::Broken);
            let query = Query {
                collections: vec!["data".into()],
                filter: None,
                fts: Some(FtsQuery {
                    pattern_parameter: None,
                    mode: objekte_search_core::query::FtsMode::Literal,
                    case_sensitivity: objekte_search_core::query::CaseSensitivity::Sensitive,
                    literal: "recovery-token".into(),
                    fields: vec![],
                }),
                options: QueryOptions {
                    sort: vec![],
                    limit: 5,
                    offset: 0,
                    count: true,
                    return_mode: ReturnMode::Locator,
                    evidence: true,
                },
            };
            assert_eq!(repo.search(&query)?.count, Some(400));
            measure(&repo, "mixed-raw-fts-build", &query, 11)?;
            repo.save_snapshot(&objekte_search_core::storage::snapshot_path(
                &source_root,
                build_options.bigram_alphabet,
            ))?;
            let started = Instant::now();
            let restored = Repository::open_with_options(
                &source_root,
                vec![definition],
                RepositoryOptions {
                    snapshot_mode: SnapshotMode::IfValid,
                    ..build_options
                },
            )?;
            let restore_ns = started.elapsed().as_nanos() as u64;
            assert!(restored.startup().restored);
            assert_eq!(restored.search(&query)?.hits, repo.search(&query)?.hits);
            println!(
                "{}",
                serde_json::json!({"case":"mixed-restore","restore_ns":restore_ns,"peak_resident_bytes":memory_bytes("VmHWM:")})
            );
            measure(&restored, "mixed-raw-fts-restore", &query, 11)?;
        }
    }
    Ok(())
}

fn measure(repo: &Repository, name: &str, query: &Query, repetitions: usize) -> Result<(), String> {
    measure_observed(repo, name, query, repetitions, None)
}

fn measure_posting_values(
    repo: &Repository,
    name: &str,
    request: &PostingValuesRequest,
) -> Result<(), String> {
    let expected = repo.posting_values(request)?;
    let mut samples = Vec::new();
    for _ in 0..31 {
        let started = Instant::now();
        let page = repo.posting_values(request)?;
        samples.push(started.elapsed().as_nanos() as u64);
        assert_eq!(page, expected);
    }
    samples.sort_unstable();
    println!(
        "{}",
        serde_json::json!({
            "case":name,
            "median_ns":samples[15],
            "samples_ns":samples,
            "visited_keys":expected.visited_keys,
            "source_reads":0,
            "values":expected.values.len(),
            "selection_records":expected.coverage.selected_records,
        })
    );
    Ok(())
}

fn measure_observed(
    repo: &Repository,
    name: &str,
    query: &Query,
    repetitions: usize,
    observation: Option<(&AtomicU64, &AtomicU64, &AtomicU64)>,
) -> Result<(), String> {
    let expected = repo.search_reference(query)?;
    let before = observation.map(|(callbacks, mappings, preparations)| {
        (
            callbacks.load(Ordering::Relaxed),
            mappings.load(Ordering::Relaxed),
            preparations.load(Ordering::Relaxed),
        )
    });
    let mut samples = Vec::new();
    let mut counters = None;
    for _ in 0..repetitions {
        let started = Instant::now();
        let result = repo.search(query)?;
        samples.push(started.elapsed().as_nanos() as u64);
        if result.count != expected.count || result.hits != expected.hits {
            return Err(format!(
                "{name}: indexed result differs from independent scanner"
            ));
        }
        counters = Some(result.counters);
    }
    samples.sort_unstable();
    let observed = observation.zip(before).map(
        |((callbacks, mappings, preparations), (old_callbacks, old_mappings, old_preparations))| {
            serde_json::json!({
                "callbacks": callbacks.load(Ordering::Relaxed) - old_callbacks,
                "query_mappings": mappings.load(Ordering::Relaxed) - old_mappings,
                "preparations": preparations.load(Ordering::Relaxed) - old_preparations,
            })
        },
    );
    println!(
        "{}",
        serde_json::json!({
            "case": name,
            "repetitions": repetitions,
            "median_ns": samples[samples.len() / 2],
            "min_ns": samples[0],
            "max_ns": samples[samples.len() - 1],
            "samples_ns": samples,
            "count": expected.count,
            "page_size": expected.hits.len(),
            "counters": counters.unwrap(),
            "peak_resident_bytes": memory_bytes("VmHWM:"),
            "instrumentation": observed,
        })
    );
    Ok(())
}

fn run() -> Result<(), String> {
    let root =
        std::env::temp_dir().join(format!("objekte-corrective-bench-{}", std::process::id()));
    fs::create_dir_all(root.join("range")).map_err(|e| e.to_string())?;
    fs::create_dir_all(root.join("left")).map_err(|e| e.to_string())?;
    fs::create_dir_all(root.join("right")).map_err(|e| e.to_string())?;
    let mut numeric = String::new();
    for num in 0..10_000 {
        numeric.push_str(&format!(
            r#"{{"num":{num},"kind":"yes","key":"v{num:05}"}}"#
        ));
        numeric.push('\n');
    }
    fs::write(root.join("range/data.jsonl"), numeric).map_err(|e| e.to_string())?;
    let padding = "x".repeat(64);
    for name in ["left", "right"] {
        let mut body = String::new();
        for num in 0..2_000 {
            let rare = if num % 500 == 0 { " rare-token" } else { "" };
            body.push_str(&format!(
                r#"{{"body":"needle{rare}","padding":"{padding}"}}"#
            ));
            body.push('\n');
        }
        fs::write(root.join(format!("{name}/data.jsonl")), body).map_err(|e| e.to_string())?;
    }
    let mut schema = Schema::default();
    for (path, kind) in [
        ("num", LogicalType::Number),
        ("kind", LogicalType::String),
        ("key", LogicalType::String),
    ] {
        schema.register_field(FieldDefinition {
            path: vec![path.into()],
            kind,
            required: true,
            nullable: false,
            indexed: path != "kind",
            multi: false,
            element_nullable: Vec::new(),
        })?;
    }
    let repo = Repository::open_with_options(
        &root,
        vec![
            definition("range", SourceFormat::JsonLines, schema, true),
            definition("left", SourceFormat::JsonLines, Schema::default(), false),
            definition("right", SourceFormat::JsonLines, Schema::default(), false),
        ],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            query_workers: 4,
            ..Default::default()
        },
    )?;
    repo.configure_verified_cache(0)?;
    let mut range = Query {
        collections: vec!["range".into()],
        fts: None,
        filter: Some(parse_filter("num >= 9990 && num <= 9998")?),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    measure(&repo, "high-cardinality-range", &range, 31)?;
    range.filter = Some(parse_filter("kind = 'yes'")?);
    range.options.sort = vec![SortField {
        field: "num".into(),
        descending: false,
        expression: None,
    }];
    range.options.limit = 5;
    range.options.offset = 7;
    range.options.count = false;
    range.options.offset = 1_000;
    measure(&repo, "page-only-stored-order-residual", &range, 31)?;
    range.options.count = true;
    measure(&repo, "counted-stored-order-residual", &range, 31)?;
    range.filter = Some(parse_filter("num = 1234")?);
    range.options.sort.clear();
    range.options.offset = 0;
    range.options.limit = 1;
    range.options.return_mode = ReturnMode::Logical;
    measure(&repo, "direct-exact-logical", &range, 31)?;
    let mut values = PostingValuesRequest::new("range", "key", 10);
    values.prefix = Some("v009".into());
    let page = repo.posting_values(&values)?;
    assert_eq!(page.values.len(), 10);
    assert_eq!(page.values[0].value, ScalarKey::String("v00900".into()));
    assert_eq!(page.values[9].value, ScalarKey::String("v00909".into()));
    measure_posting_values(&repo, "posting-values-prefix", &values)?;
    values.prefix = None;
    values.lower = Some(PostingValueBound {
        value: ScalarKey::String("v00900".into()),
        inclusive: true,
    });
    values.upper = Some(PostingValueBound {
        value: ScalarKey::String("v00999".into()),
        inclusive: true,
    });
    let range_page = repo.posting_values(&values)?;
    assert_eq!(range_page.values, page.values);
    measure_posting_values(&repo, "posting-values-range", &values)?;
    let selection = repo.select_exact(
        "range",
        &Query {
            collections: vec!["range".into()],
            filter: Some(parse_filter("num >= 900 && num < 950")?),
            fts: None,
            options: QueryOptions {
                limit: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: false,
                offset: 0,
                sort: vec![],
            },
        },
    )?;
    values.lower = None;
    values.upper = None;
    values.prefix = Some("v009".into());
    values.selection = Some(selection);
    let selected = repo.posting_values(&values)?;
    assert_eq!(selected.values.len(), 10);
    assert!(selected.values.iter().all(|entry| entry.count == 1));
    measure_posting_values(&repo, "posting-values-selected", &values)?;
    let mut scan = Query {
        collections: vec!["left".into(), "right".into()],
        fts: Some(FtsQuery {
            pattern_parameter: None,
            mode: objekte_search_core::query::FtsMode::Literal,
            case_sensitivity: objekte_search_core::query::CaseSensitivity::Sensitive,
            literal: "needle".into(),
            fields: vec![],
        }),
        filter: None,
        options: QueryOptions {
            sort: vec![],
            limit: 5,
            offset: 7,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: false,
        },
    };
    measure(&repo, "jsonl-dense-logical", &scan, 11)?;
    scan.options.offset = 1_000;
    measure(&repo, "jsonl-deep-offset", &scan, 11)?;
    scan.options.offset = 0;
    scan.options.return_mode = ReturnMode::Locator;
    measure(&repo, "jsonl-locator", &scan, 11)?;
    scan.options.return_mode = ReturnMode::Logical;
    scan.options.limit = 0;
    measure(&repo, "jsonl-count-only", &scan, 11)?;
    scan.options.limit = 5;
    scan.fts.as_mut().unwrap().literal = "rare-token".into();
    measure(&repo, "jsonl-sparse-logical", &scan, 11)?;
    custom_and_malformed_cases(&root)?;
    fs::remove_dir_all(&root).map_err(|e| e.to_string())?;
    Ok(())
}

fn memory_bytes(name: &str) -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix(name)?
                .trim()
                .strip_suffix(" kB")?
                .trim()
                .parse::<u64>()
                .ok()
                .map(|value| value * 1_024)
        })
}
