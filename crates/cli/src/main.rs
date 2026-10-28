use objekte_core::cache;
use std::path::PathBuf;

fn help() {
    println!("objekte — command line interface for the Objekte library");
    println!();
    println!("Usage:");
    println!("  objekte cache build REPOSITORY COLLECTION [CACHE_DIRECTORY]");
    println!("  objekte cache changes REPOSITORY COLLECTION [CACHE_DIRECTORY]");
    println!("  objekte --version");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => help(),
        [arg] if arg == "--help" || arg == "-h" => help(),
        [arg] if arg == "--version" || arg == "-V" => {
            println!("objekte {}", env!("CARGO_PKG_VERSION"));
        }
        [group, action, repository, collection] | [group, action, repository, collection, _]
            if group == "cache" && (action == "build" || action == "changes") =>
        {
            let root = PathBuf::from(repository);
            let cache_root = args
                .get(4)
                .map(PathBuf::from)
                .unwrap_or_else(cache::default_cache_root);
            let result = if action == "build" {
                cache::build_collection(&root, collection, &cache_root).map(|built| {
                    println!("generation: {}", built.generation);
                    println!("records: {}", built.index.records.len());
                    println!("cache: {}", built.directory.display());
                })
            } else {
                cache::detect_changes(&root, collection, &cache_root).map(|changes| {
                    println!("changed sources: {}", changes.len());
                    for path in changes {
                        println!("{path}")
                    }
                })
            };
            if let Err(error) = result {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("Unknown command. Run 'objekte --help' for usage.");
            std::process::exit(2);
        }
    }
}
