mod args;
mod commands;
mod output;

fn main() {
    if let Err(e) = commands::run() {
        eprintln!("{e}");
        std::process::exit(2)
    }
}
