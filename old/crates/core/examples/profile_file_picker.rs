//! Candidate-index timing only; does not parse or verify query matches.
use objekte_core::file_picker::FilePicker;
use std::{path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples/musenalm/data/contents"));
    let started = Instant::now();
    let picker = FilePicker::open(&root)?;
    println!(
        "files={} index_mib={:.1} open_ms={:.1}",
        picker.len(),
        picker.index_bytes() as f64 / 1_048_576.0,
        started.elapsed().as_secs_f64() * 1_000.0,
    );
    for term in ["Liebe", "almanach", "Goethe", "Mond"] {
        let literal = [term.to_owned()];
        let started = Instant::now();
        let candidates = picker.candidate_ids(Some(&literal));
        println!(
            "{term}: candidates={} index_ms={:.3}",
            candidates.len(),
            started.elapsed().as_secs_f64() * 1_000.0,
        );
    }
    Ok(())
}
