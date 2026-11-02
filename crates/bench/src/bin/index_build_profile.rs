//! Cold, cache-free index-build profile for one repository collection.

use objekte_core::{
    BigramEncoding, Field, Repository, TextFormat, ValueType, file_picker::FilePicker,
};
use serde_json::json;
use std::{collections::BTreeMap, env, fs, io, path::PathBuf, time::Instant};

fn string_field() -> Field {
    Field {
        kind: ValueType::String,
        nullable: true,
        index: false,
        language: None,
        sort_index: false,
        sort_reduce: None,
        text_format: None,
        exclude_fts: false,
        enum_values: None,
        properties: BTreeMap::new(),
        items: None,
        relation: None,
    }
}

fn mark_text_field(
    fields: &mut BTreeMap<String, Field>,
    path: &str,
    format: TextFormat,
) -> io::Result<()> {
    let (head, tail) = path
        .split_once('.')
        .map_or((path, None), |(head, tail)| (head, Some(tail)));
    let array = head.ends_with("[]");
    let name = head.strip_suffix("[]").unwrap_or(head);
    let field = if tail.is_none() && !array {
        fields.entry(name.to_owned()).or_insert_with(string_field)
    } else {
        fields
            .get_mut(name)
            .ok_or_else(|| io::Error::other(format!("undeclared parent field {name:?}")))?
    };
    let field = if array {
        field
            .items
            .as_deref_mut()
            .ok_or_else(|| io::Error::other(format!("{name:?} has no declared array item")))?
    } else {
        field
    };
    if let Some(tail) = tail {
        mark_text_field(&mut field.properties, tail, format)
    } else if field.kind == ValueType::String {
        field.text_format = Some(format);
        Ok(())
    } else {
        Err(io::Error::other(format!("{path:?} is not a string field")))
    }
}

fn rss_kib(key: &str) -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse().ok())
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut root = None;
    let mut collection_name = None;
    let mut encoding = BigramEncoding::Ascii;
    let mut field_specs = Vec::new();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--repo" => root = args.next().map(PathBuf::from),
            "--collection" => collection_name = args.next(),
            "--encoding" => {
                encoding = match args.next().as_deref() {
                    Some("ascii") => BigramEncoding::Ascii,
                    Some("utf8_bytes") => BigramEncoding::Utf8Bytes,
                    _ => return Err("--encoding must be ascii or utf8_bytes".into()),
                };
            }
            "--field" => field_specs.push(args.next().ok_or("--field needs PATH=FORMAT")?),
            _ => return Err(format!("unknown argument {arg:?}").into()),
        }
    }
    let root = root.ok_or("--repo is required")?;
    let collection_name = collection_name.ok_or("--collection is required")?;
    let repo = Repository::open(&root)?;
    let mut collection = repo
        .model()
        .collections
        .get(&collection_name)
        .ok_or_else(|| format!("unknown collection {collection_name:?}"))?
        .clone();
    for spec in &field_specs {
        let (path, format) = spec.split_once('=').ok_or("--field needs PATH=FORMAT")?;
        let format = match format {
            "plain" => TextFormat::Plain,
            "markdown" => TextFormat::Markdown,
            "html" => TextFormat::Html,
            "xml" => TextFormat::Xml,
            _ => return Err(format!("unsupported text format {format:?}").into()),
        };
        mark_text_field(&mut collection.fields, path, format)?;
    }

    let started = Instant::now();
    let picker = FilePicker::profile_collection(&collection, encoding)?;
    let build_wall_nanos = started.elapsed().as_nanos();
    let build_peak_rss_kib = rss_kib("VmHWM:");
    let build_ready_rss_kib = rss_kib("VmRSS:");
    let index_sizes = picker.profile_index_sizes();
    let build_timings = picker.build_timings();

    let snapshot_dir = tempfile::tempdir_in("target")?;
    let snapshot = snapshot_dir.path().join("index.bigram");
    let save_started = Instant::now();
    picker.save_bigram_snapshot(&snapshot)?;
    let snapshot_save_wall_nanos = save_started.elapsed().as_nanos();
    let secondary_serialize = picker.profile_secondary_snapshot(&snapshot)?;
    let snapshot_base_bytes = fs::metadata(&snapshot)?.len();
    let snapshot_secondary_bytes = fs::metadata(snapshot.with_extension("secondary"))?.len();
    let restore_started = Instant::now();
    let restored = FilePicker::profile_restore_collection(&collection, encoding, &snapshot)?;
    let snapshot_restore_wall_nanos = restore_started.elapsed().as_nanos();
    if restored.len() != picker.len()
        || serde_json::to_value(restored.profile_index_sizes())?
            != serde_json::to_value(&index_sizes)?
    {
        return Err("restored index differs from the built index".into());
    }

    println!(
        "{}",
        json!({
            "repo": root,
            "collection": collection_name,
            "documents": picker.len(),
            "encoding": encoding,
            "fields": field_specs,
            "thread_cpu_supported": cfg!(unix),
            "build_wall_nanos": build_wall_nanos,
            "build_peak_rss_kib": build_peak_rss_kib,
            "build_ready_rss_kib": build_ready_rss_kib,
            "compressed_index_bytes": picker.index_bytes(),
            "index_sizes": index_sizes,
            "stages": build_timings,
            "snapshot_save_wall_nanos": snapshot_save_wall_nanos,
            "snapshot_restore_wall_nanos": snapshot_restore_wall_nanos,
            "secondary_serialize": secondary_serialize,
            "snapshot_base_bytes": snapshot_base_bytes,
            "snapshot_secondary_bytes": snapshot_secondary_bytes,
        })
    );
    Ok(())
}
