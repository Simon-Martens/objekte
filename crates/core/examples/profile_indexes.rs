//! Reproducible view/query comparison across library revisions.
//! Usage: profile_indexes REPOSITORY DATA_DIR WORKLOAD.json [REPEATS]
use objekte_core::{JsonQuery, Options, Repository, ViewRequest};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{env, error::Error, fs, time::Instant};


#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Case {
    View {
        name: String,
        collection: String,
        request: ViewRequest,
    },
    Query {
        name: String,
        request: JsonQuery,
    },
}


impl Case {
    fn name(&self) -> &str {
        match self {
            Self::View { name, .. } | Self::Query { name, .. } => name,
        }
    }

    fn run(&self, repo: &mut Repository) -> Result<Value, Box<dyn Error>> {
        let start = Instant::now();
        let (result, metrics, plan) = match self {
            Self::View {
                collection,
                request,
                ..
            } => {
                let r = repo.open_view(collection, request)?;
                (
                    json!({"rows":r.rows,"total":r.total}),
                    r.metrics,
                    r.query_plan,
                )
            }
            Self::Query { request, .. } => {
                let r = repo.query_json(request)?;
                let records = r.records.iter().map(|r| json!({"collection":r.collection,"id":r.id,"path":r.path,"data":r.data})).collect::<Vec<_>>();
                (
                    json!({"records":records,"total":r.total}),
                    r.metrics,
                    r.query_plan,
                )
            }
        };
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&result)?));
        let mut converted = serde_json::Map::new();
        for (key, value) in serde_json::to_value(metrics)?.as_object().unwrap() {
            if let Some(prefix) = key.strip_suffix("_nanos") {
                converted.insert(
                    format!("{prefix}_ms"),
                    json!(value.as_u64().unwrap() as f64 / 1e6),
                );
            } else {
                converted.insert(key.clone(), value.clone());
            }
        }
        Ok(
            json!({"case":self.name(),"elapsed_ms":ms,"digest":digest,"metrics":converted,"plan":plan}),
        )
    }
}


fn main() -> Result<(), Box<dyn Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if !(3..=4).contains(&args.len()) {
        return Err("usage: profile_indexes REPOSITORY DATA_DIR WORKLOAD.json [REPEATS]".into());
    }
    let repeats = args
        .get(3)
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(5);
    if repeats == 0 {
        return Err("REPEATS must be positive".into());
    }
    let cases: Vec<Case> = serde_json::from_slice(&fs::read(&args[2])?)?;
    let mut options = Options::new(&args[1]);
    options.ingest_workers = 4;
    options.background_workers = 4;
    options.sqlite_cache_mib = Some(64);
    let mut repo = Repository::load_repository(&args[0], options.clone())?;
    let mut digests = Vec::new();
    for case in &cases {
        let mut first = case.run(&mut repo)?;
        first["phase"] = json!("first");
        digests.push(first["digest"].clone());
        println!("{first}");
        for i in 0..repeats {
            let mut warm = case.run(&mut repo)?;
            assert_eq!(warm["digest"], first["digest"], "{}", case.name());
            assert_eq!(warm["metrics"]["source_reads"], 0);
            assert_eq!(warm["metrics"]["enumerations"], 0);
            warm["phase"] = json!("warm");
            warm["iteration"] = json!(i);
            println!("{warm}");
        }
    }
    drop(repo);
    let mut repo = Repository::load_repository(&args[0], options)?;
    for (case, expected) in cases.iter().zip(digests) {
        let mut reopened = case.run(&mut repo)?;
        assert_eq!(reopened["digest"], expected, "{} after reopen", case.name());
        assert_eq!(reopened["metrics"]["source_reads"], 0);
        reopened["phase"] = json!("reopen");
        println!("{reopened}");
    }
    Ok(())
}
