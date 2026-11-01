//! Measure snapshot decoding separately from publishing a repository picker.

use objekte_core::{Repository, bigram::BigramSnapshot};
use std::{env, error::Error, path::Path, time::Instant};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let root = args
        .next()
        .ok_or("usage: snapshot_restore REPOSITORY COLLECTION [RUNS] [--save]")?;
    let name = args.next().ok_or("missing collection")?;
    let mut runs = 3;
    let mut save = false;
    for argument in args {
        if argument == "--save" {
            save = true;
        } else {
            runs = argument.parse::<usize>()?;
        }
    }
    for run in 1..=runs {
        let mut repository = Repository::open(Path::new(&root))?;
        let collection = repository
            .model()
            .collections
            .get(&name)
            .ok_or("unknown collection")?;
        let cache = repository.bigram_cache_path(&name)?;
        let started = Instant::now();
        let snapshot = BigramSnapshot::load_scoped(&cache, &collection.path, collection.recursive)?;
        let decode_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let files = snapshot.files.len();
        let save_ms = if save {
            let temporary = tempfile::tempdir()?;
            let started = Instant::now();
            snapshot.save(&temporary.path().join("snapshot.bin"))?;
            format!("{:.2}", started.elapsed().as_secs_f64() * 1_000.0)
        } else {
            "-".into()
        };
        drop(snapshot);
        let started = Instant::now();
        let restored = repository.restore_cached_index_unchecked(&name)?;
        let publish_ms = started.elapsed().as_secs_f64() * 1_000.0;
        if !restored {
            return Err("cache could not be restored without an inventory".into());
        }
        println!(
            "run={run} files={files} snapshot_decode_ms={decode_ms:.2} publish_ms={publish_ms:.2} save_ms={save_ms} secondary_loaded={}",
            repository.metrics().secondary_snapshot_loads,
        );
    }
    Ok(())
}
