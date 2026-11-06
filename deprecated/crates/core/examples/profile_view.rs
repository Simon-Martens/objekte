//! Profile the same view repeatedly through one library session, then reopen it.
//! Usage: profile_view REPOSITORY COLLECTION [REQUESTS] [REQUEST.json]
use objekte_core::{Options, Repository, ViewRequest};
use serde_json::json;
use std::{env, error::Error, fs, time::Instant};

fn main() -> Result<(), Box<dyn Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if !(2..=4).contains(&args.len()) {
        return Err("usage: profile_view REPOSITORY COLLECTION [REQUESTS] [REQUEST.json]".into());
    }
    let repeats = args
        .get(2)
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(5);
    if repeats == 0 {
        return Err("REQUESTS must be positive".into());
    }
    let request: ViewRequest = match args.get(3) {
        Some(path) => serde_json::from_slice(&fs::read(path)?)?,
        None => ViewRequest::default(),
    };
    let options = Options::new();
    let t = Instant::now();
    let mut repo = Repository::load_repository(&args[0], options.clone())?;
    let load_ms = t.elapsed().as_secs_f64() * 1000.0;
    let indexes = &repo.model().collection(&args[1])?.indexes;
    println!(
        "{}",
        json!({"phase":"load", "elapsed_ms":load_ms, "indexes":indexes})
    );
    let mut expected = None;
    for iteration in 0..repeats {
        let t = Instant::now();
        let view = repo.open_view(&args[1], &request)?;
        let elapsed_ms = t.elapsed().as_secs_f64() * 1000.0;
        if let Some(rows) = &expected {
            assert_eq!(&view.rows, rows, "repeated request returned different rows");
        } else {
            expected = Some(view.rows.clone());
        }
        println!(
            "{}",
            json!({"phase":"view", "iteration":iteration, "elapsed_ms":elapsed_ms, "rows":view.rows.len(), "metrics":view.metrics, "query_plan":view.query_plan})
        );
    }
    drop(repo);
    let t = Instant::now();
    let mut reopened = Repository::load_repository(&args[0], options)?;
    let load_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let view = reopened.open_view(&args[1], &request)?;
    let elapsed_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(
        Some(&view.rows),
        expected.as_ref(),
        "reopen returned different rows"
    );
    println!(
        "{}",
        json!({"phase":"reopen", "load_ms":load_ms, "elapsed_ms":elapsed_ms, "rows":view.rows.len(), "metrics":view.metrics, "query_plan":view.query_plan})
    );
    Ok(())
}
