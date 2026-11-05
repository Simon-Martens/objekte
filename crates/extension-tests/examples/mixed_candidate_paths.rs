//! Focused, repeatable public-query measurements for mixed candidate routes.

use objekte_core::{
    CacheMode, JsonQuery, JsonQueryBackend, Options, Repository, Result,
    extensions::{
        SourceDescriptor, SourceProvider,
        identity::{DataKind, Dependency, Descriptor, SemanticVersion},
    },
};
use objekte_extension_tests::MarkdownProvider;
use serde_json::json;
use std::{env, fs, path::Path, time::Instant};


struct JsonAliasProvider {
    inner: MarkdownProvider,
    descriptor: Descriptor,
}


impl JsonAliasProvider {
    fn new(root: &Path) -> Self {
        Self {
            inner: MarkdownProvider::new(root.to_path_buf()),
            descriptor: Descriptor::new(
                "example:bench-json-alias",
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


fn prepare(root: &Path, count: usize) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if root.exists() {
        return Err(format!("fixture already exists: {}", root.display()).into());
    }
    let mixed = root.join("data/mixed");
    fs::create_dir_all(mixed.join(".editor"))?;
    fs::write(
        mixed.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"body":{"type":"string","index":true,"x-full-text":"plain"},"year":{"type":"integer","index":true}}}"#,
    )?;
    for index in 0..count {
        let year = if index % 3 == 2 { 1850 } else { 1848 };
        let body = if index == 0 || index == count / 2 || index + 1 == count {
            "common raretokenXYZ"
        } else {
            "common filler"
        };
        fs::write(
            mixed.join(format!("n{index:05}.json")),
            format!(r#"{{"year":{year},"body":"{body}"}}"#),
        )?;
    }
    fs::write(
        mixed.join("z.md"),
        r#"{"year":1848,"body":"common raretokenXYZ custom"}"#,
    )?;
    Ok(())
}

fn open(root: &Path, cache: CacheMode) -> Result<Repository> {
    Repository::builder(root)
        .options(Options {
            cache,
            query_cache_bytes: 0,
            ..Default::default()
        })
        .register_source(JsonAliasProvider::new(&root.join("data/mixed")))?
        .open()
}

fn request(scenario: &str) -> JsonQuery {
    let (filter, backend) = match scenario {
        "direct-fallback" => (
            "body:fts ~ 'common' && year = 1848",
            JsonQueryBackend::Direct,
        ),
        "indexed-fallback" => ("body:fts ~ 'common' && year = 1848", JsonQueryBackend::Auto),
        "rare-fts" => ("body:fts ~ 'raretokenXYZ'", JsonQueryBackend::Auto),
        _ => unreachable!(),
    };
    JsonQuery {
        filter: filter.into(),
        backend,
        collections: Some(vec!["mixed".into()]),
        limit: 1,
        count: true,
        ..Default::default()
    }
}

fn run(root: &Path, rounds: usize) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut repo = open(root, CacheMode::Memory)?;
    let warm = repo.warm_bigram_index("mixed")?;
    println!("{}", json!({"type":"setup","warm":warm}));
    let mut oracle = open(root, CacheMode::Disabled)?;
    for scenario in ["direct-fallback", "indexed-fallback", "rare-fts"] {
        let query = request(scenario);
        let expected = oracle.query_json(&JsonQuery {
            backend: JsonQueryBackend::Direct,
            ..query.clone()
        })?;
        let started = Instant::now();
        let actual = repo.query_json(&query)?;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        if actual.total != expected.total
            || actual.truncated != expected.truncated
            || actual.records.iter().map(|r| &r.id).collect::<Vec<_>>()
                != expected.records.iter().map(|r| &r.id).collect::<Vec<_>>()
        {
            return Err(format!("{scenario} differs from direct oracle").into());
        }
        println!(
            "{}",
            json!({"type":"initial","scenario":scenario,"ms":ms,
                "total":actual.total,"first_id":actual.records.first().map(|r|&r.id),
                "metrics":actual.metrics,"plan":actual.query_plan})
        );
    }
    for round in 0..rounds {
        let order = if round % 2 == 0 {
            ["rare-fts", "direct-fallback", "indexed-fallback"]
        } else {
            ["indexed-fallback", "direct-fallback", "rare-fts"]
        };
        for scenario in order {
            let started = Instant::now();
            let result = repo.query_json(&request(scenario))?;
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            println!(
                "{}",
                json!({"type":"sample","round":round,"scenario":scenario,"ms":ms,
                    "total":result.total,"truncated":result.truncated,
                    "first_id":result.records.first().map(|r|&r.id),
                    "metrics":result.metrics,"plan":result.query_plan})
            );
        }
    }
    Ok(())
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 4 {
        return Err("usage: mixed_candidate_paths prepare|run ROOT COUNT_OR_ROUNDS".into());
    }
    let root = Path::new(&args[2]);
    let n = args[3].parse::<usize>()?;
    match args[1].as_str() {
        "prepare" => prepare(root, n),
        "run" => run(root, n),
        _ => Err("expected prepare or run".into()),
    }
}
