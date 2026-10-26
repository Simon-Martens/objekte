use objekte_core as _;

fn main() {
    let mut arguments = std::env::args().skip(1);

    match (arguments.next().as_deref(), arguments.next()) {
        (None, _) | (Some("--help" | "-h"), None) => {
            println!("objekte — command line interface for the Objekte library");
            println!();
            println!("Usage: objekte [COMMAND]");
            println!();
            println!("No project commands have been added yet.");
        }
        (Some("--version" | "-V"), None) => {
            println!("objekte {}", env!("CARGO_PKG_VERSION"));
        }
        _ => {
            eprintln!("Unknown command. Run 'objekte --help' for usage.");
            std::process::exit(2);
        }
    }
}
