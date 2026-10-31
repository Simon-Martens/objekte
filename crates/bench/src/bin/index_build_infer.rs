//! Time process-local index construction, including optional inferred scalar fields.

use objekte_core::{Options, Repository};
use serde_json::json;
use std::{env, time::Instant};


fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let root = args
        .next()
        .ok_or("usage: index-build-infer REPOSITORY [--infer]")?;
    let infer = match args.next().as_deref() {
        None => false,
        Some("--infer") => true,
        Some(_) => return Err("usage: index-build-infer REPOSITORY [--infer]".into()),
    };
    let mut options = Options::new();
    options.infer_field_types = infer;
    let mut repo = Repository::load_repository(root, options)?;
    let collections = repo.model().collections.keys().cloned().collect::<Vec<_>>();
    let total_started = Instant::now();
    for collection in &collections {
        let started = Instant::now();
        let metrics = repo.warm_bigram_index(collection)?;
        println!(
            "{}",
            json!({
                "type": "collection",
                "collection": collection,
                "infer": infer,
                "wall_ms": started.elapsed().as_secs_f64() * 1_000.0,
                "index_ms": metrics.bigram_open_nanos as f64 / 1_000_000.0,
            })
        );
    }
    println!(
        "{}",
        json!({
            "type": "total",
            "infer": infer,
            "collections": collections.len(),
            "wall_ms": total_started.elapsed().as_secs_f64() * 1_000.0,
        })
    );
    Ok(())
}
