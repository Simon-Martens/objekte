//! Release benchmark for schema-bound full-text index and mixed searches.

use objekte_core::{JsonQuery, JsonQueryBackend, Repository, TextMode, TextQuery};
use serde_json::{Value, json};
use std::{
    env, fs,
    io::Write,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};


/// One filter or text-search request measured after both indexes are warm.
struct Case {
    id: usize,
    kind: &'static str,
    query: &'static str,
    field: Option<&'static str>,
    mode: Option<TextMode>,
}


/// Wall time, total matches, and source work for one request.
struct Sample {
    ms: f64,
    matches: usize,
    candidates: u64,
    scans: u64,
    parses: u64,
}


fn mib(key: &str) -> f64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|value| value.parse::<f64>().ok())
            })
        })
        .unwrap_or(0.0)
        / 1024.0
}

fn cases() -> Vec<Case> {
    let filters = [
        "annotation:fts ~ 'Goet'",
        "annotation:fts ~ 'Schill'",
        "annotation:fts ~ 'Mond'",
        "annotation:fts ~ :ignorecase('GOET')",
        "annotation:fts ~ 'Liebe'",
        "year = 1822 && annotation:fts ~ 'Goet'",
        "year >= 1800 && annotation:fts ~ 'Wien'",
        "year < 1800 && annotation:fts ~ 'Berlin'",
        "annotation:fts ~ 'Heinrich' || year = 1822",
        "annotation:fts !~ 'Goet'",
        "preferred_title:fts ~ 'Goethe'",
        "preferred_title:fts ~ 'Schiller'",
        "preferred_title:fts ~ :ignorecase('goethe')",
        "preferred_title:fts ~ 'Gedicht'",
        "preferred_title:fts ~ 'A%buch'",
        "title_statement:fts ~ 'Verlag'",
        "title_statement:fts ~ :ignorecase('berlin')",
        "contributors.annotation:fts ?~ 'Goet'",
        "annotation:fts = 'Goethe'",
        "annotation:fts ~ 'XYZunlikely'",
    ];
    let mut cases = filters
        .into_iter()
        .enumerate()
        .map(|(index, query)| Case {
            id: index + 1,
            kind: "filter",
            query,
            field: None,
            mode: None,
        })
        .collect::<Vec<_>>();
    let searches = [
        ("Goethe", Some("annotation"), TextMode::Word),
        ("Schiller", Some("annotation"), TextMode::Word),
        ("Goethe", Some("preferred_title"), TextMode::Word),
        ("Liebe", Some("preferred_title"), TextMode::Word),
        ("Goethe", None, TextMode::Word),
        ("Goet.*he", Some("annotation"), TextMode::RegexClean),
        ("Schill", Some("preferred_title"), TextMode::RegexClean),
        (r"\*\*[^*]+\*\*", Some("annotation"), TextMode::RegexRaw),
        (r"<[^>]+>", Some("title_statement"), TextMode::RegexRaw),
        ("Wien|Berlin", None, TextMode::RegexClean),
    ];
    cases.extend(
        searches
            .into_iter()
            .enumerate()
            .map(|(index, (query, field, mode))| Case {
                id: filters.len() + index + 1,
                kind: match mode {
                    TextMode::Word => "word",
                    TextMode::WordRaw => "word_raw",
                    TextMode::RegexClean => "regex_clean",
                    TextMode::RegexRaw => "regex_raw",
                },
                query,
                field,
                mode: Some(mode),
            }),
    );
    cases
}

fn run(repo: &mut Repository, case: &Case) -> Result<Sample, Box<dyn std::error::Error>> {
    let started = Instant::now();
    let (matches, metrics) = if let Some(mode) = case.mode {
        let result = repo.search_text(&TextQuery {
            pattern: case.query.into(),
            mode,
            collections: Some(vec!["contents".into()]),
            fields: case.field.map(|field| vec![field.into()]),
            count: true,
            limit: 1,
            ..Default::default()
        })?;
        (result.total.unwrap_or(0), result.metrics)
    } else {
        let result = repo.query_json(&JsonQuery {
            filter: case.query.into(),
            collections: Some(vec!["contents".into()]),
            count: true,
            limit: 1,
            backend: JsonQueryBackend::Bigram,
            ..Default::default()
        })?;
        (result.total.unwrap_or(0), result.metrics)
    };
    Ok(Sample {
        ms: started.elapsed().as_secs_f64() * 1000.0,
        matches,
        candidates: metrics.fts_candidates,
        scans: metrics.grep_scans,
        parses: metrics.parses,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples/musenalm"));
    let runs: usize = env::args().nth(2).unwrap_or_else(|| "3".into()).parse()?;
    if runs == 0 {
        return Err("runs must be positive".into());
    }
    let original = root.join(".objekte/collections/contents.schema.json");
    let mut schema: Value = serde_json::from_slice(&fs::read(original)?)?;
    schema["properties"]["annotation"]["x-full-text"] = json!("markdown");
    schema["properties"]["preferred_title"]["x-full-text"] = json!("plain");
    schema["properties"]["title_statement"]["x-full-text"] = json!("html");
    schema["properties"]["contributors"]["items"]["properties"]["annotation"]["x-full-text"] =
        json!("xml");
    let mut temporary_schema = tempfile::NamedTempFile::new()?;
    temporary_schema.write_all(serde_json::to_string(&schema)?.as_bytes())?;
    temporary_schema.flush()?;
    let mut repo = Repository::builder(&root)
        .without_default_schemas()
        .schema("contents", temporary_schema.path())
        .open()?;
    let opened_rss_mib = mib("VmRSS:");
    let base_started = Instant::now();
    repo.warm_bigram_index("contents")?;
    let base_ms = base_started.elapsed().as_secs_f64() * 1000.0;
    thread::sleep(Duration::from_millis(250));
    let base_rss_mib = mib("VmRSS:");
    let text_built_with_base = repo.text_index_bytes("contents")?.is_some();
    let base_bytes = repo.index_bytes("contents")?.unwrap_or(0)
        - repo.text_index_bytes("contents")?.unwrap_or(0);
    let fts_started = Instant::now();
    repo.warm_text_index("contents")?;
    let fts_ms = fts_started.elapsed().as_secs_f64() * 1000.0;
    thread::sleep(Duration::from_millis(250));
    let ready_rss_mib = mib("VmRSS:");
    let ready_peak_rss_mib = mib("VmHWM:");
    let text_bytes = repo.text_index_bytes("contents")?.unwrap_or(0);
    let total_index_bytes = repo.index_bytes("contents")?.unwrap_or(0);
    let cases = cases();
    eprintln!(
        "initial index build {base_ms:.1} ms; explicit FTS warm {fts_ms:.1} ms; running {} cases x {runs}",
        cases.len()
    );
    println!(
        "{}",
        json!({
            "type":"setup", "repo":root, "collection":"contents", "cases":cases.len(), "runs":runs,
            "schema_overlay":{"annotation":"markdown","preferred_title":"plain","title_statement":"html","contributors[].annotation":"xml"},
            "fts_built_with_base":text_built_with_base,
            "combined_index_build_ms":base_ms+fts_ms,
            "base_index_build_ms":base_ms, "fts_index_build_ms":fts_ms,
            "rss_open_mib":opened_rss_mib, "rss_base_ready_mib":base_rss_mib,
            "rss_fts_ready_mib":ready_rss_mib, "rss_fts_delta_mib":ready_rss_mib-base_rss_mib,
            "peak_rss_after_build_mib":ready_peak_rss_mib,
            "base_index_bytes":base_bytes, "fts_index_bytes":text_bytes,
            "total_index_bytes":total_index_bytes,
            "count_all":true, "page_limit":1
        })
    );
    let expected = cases
        .iter()
        .map(|case| run(&mut repo, case).map(|sample| sample.matches))
        .collect::<Result<Vec<_>, _>>()?;
    let mut samples = (0..cases.len())
        .map(|_| Vec::new())
        .collect::<Vec<Vec<Sample>>>();
    for round in 0..runs {
        let order = if round % 2 == 0 {
            (0..cases.len()).collect::<Vec<_>>()
        } else {
            (0..cases.len()).rev().collect::<Vec<_>>()
        };
        for index in order {
            let sample = run(&mut repo, &cases[index])?;
            if sample.matches != expected[index] {
                return Err(format!("case {} changed its count", cases[index].id).into());
            }
            samples[index].push(sample);
        }
        eprintln!("completed round {}/{runs}", round + 1);
    }
    for (case, mut runs) in cases.iter().zip(samples) {
        runs.sort_by(|left, right| left.ms.total_cmp(&right.ms));
        let median = &runs[runs.len() / 2];
        println!(
            "{}",
            json!({
                "type":"case", "id":case.id, "kind":case.kind, "query":case.query, "field":case.field,
                "matches":median.matches, "median_ms":median.ms, "min_ms":runs.first().unwrap().ms,
                "max_ms":runs.last().unwrap().ms, "fts_candidates":median.candidates,
                "scans":median.scans, "parses":median.parses
            })
        );
    }
    Ok(())
}
