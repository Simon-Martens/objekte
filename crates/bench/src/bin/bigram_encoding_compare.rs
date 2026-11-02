//! One-process measurement of one encoding; run twice for comparable peak RSS.

use objekte_core::{BigramEncoding, bigram::BigramSnapshot, file_picker::FilePicker};
use serde_json::json;
use std::{env, fs, path::Path, time::Instant};

fn peak_rss_kib() -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
}

fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn synthetic(root: &Path, count: usize) -> std::io::Result<()> {
    const TITLES: &[&str] = &[
        "Müller",
        "Straße",
        "Äpfel",
        "français",
        "œuvre",
        "niño",
        "Łódź",
        "Привет",
        "中文",
        "日本語",
        "مرحبا",
        "😀🐈",
    ];
    fs::create_dir_all(root)?;
    for slot in 0..count {
        let title = TITLES[slot % TITLES.len()];
        let value =
            json!({"title": title, "id": slot, "body": format!("record {slot} from {title}")});
        fs::write(root.join(format!("{slot:07}.json")), value.to_string())?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let encoding = BigramEncoding::parse(&args.next().ok_or("expected encoding")?)
        .ok_or("expected ascii or utf8_bytes")?;
    let dataset = args.next().ok_or("expected a directory or synthetic:N")?;
    let synthetic_dir;
    let path = if let Some(count) = dataset.strip_prefix("synthetic:") {
        synthetic_dir = tempfile::tempdir()?;
        synthetic(&synthetic_dir.path().join("data"), count.parse()?)?;
        synthetic_dir.path().join("data")
    } else {
        Path::new(&dataset).canonicalize()?
    };
    let started = Instant::now();
    let picker = FilePicker::open_with_encoding(&path, encoding)?;
    let build_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let build_peak_rss_kib = peak_rss_kib();
    let compressed_bytes = picker.index_bytes();
    let snapshot_dir = tempfile::tempdir()?;
    let snapshot_path = snapshot_dir.path().join("index.bigram");
    let started = Instant::now();
    picker.save_bigram_snapshot(&snapshot_path)?;
    let save_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let snapshot_bytes = fs::metadata(&snapshot_path)?.len();
    let started = Instant::now();
    let restored = BigramSnapshot::load(&snapshot_path)?;
    let restore_ms = started.elapsed().as_secs_f64() * 1_000.0;
    assert_eq!(restored.filter.encoding(), encoding);
    let mut queries = Vec::new();
    for term in [
        "Wasser",
        "Berlin",
        "Müller",
        "Straße",
        "français",
        "中文",
        "日本語",
    ] {
        let literal = [term.to_owned()];
        let count = picker.candidate_ids(Some(&literal)).len();
        let mut filter_timings = Vec::with_capacity(21);
        for _ in 0..21 {
            let started = Instant::now();
            let _ = restored.filter.query(term.as_bytes());
            filter_timings.push(started.elapsed().as_secs_f64() * 1_000.0);
        }
        let mut timings = Vec::with_capacity(21);
        for _ in 0..21 {
            let started = Instant::now();
            let _ = picker.candidate_ids(Some(&literal));
            timings.push(started.elapsed().as_secs_f64() * 1_000.0);
        }
        queries.push(json!({"term":term,"candidates":count,"filter_ms":median(&mut filter_timings),"median_ms":median(&mut timings)}));
    }
    println!(
        "{}",
        json!({
            "encoding":encoding.as_str(), "dataset":dataset, "files":picker.len(),
            "build_ms":build_ms, "build_peak_rss_kib":build_peak_rss_kib,
            "compressed_bytes":compressed_bytes, "snapshot_bytes":snapshot_bytes,
            "save_ms":save_ms, "restore_ms":restore_ms, "queries":queries,
        })
    );
    Ok(())
}
