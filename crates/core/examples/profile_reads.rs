//! Read-only timings against a real project; run with --release for production timings.
use std::time::Instant;
use structured_editor_core::{Editor, ListRequest};

fn timed<T>(name: &str, work: impl FnOnce() -> T) -> T {
    eprintln!("{name}: starting");
    let start = Instant::now();
    let value = work();
    eprintln!("{name}: {:.3} ms", start.elapsed().as_secs_f64() * 1000.0);
    value
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let root = args
        .get(1)
        .expect("usage: profile_reads PROJECT COLLECTION [ID]");
    let name = args.get(2).expect("collection");
    let editor = timed("open", || Editor::open(std::path::Path::new(root)))?;
    if let Some(id) = args.get(3) {
        let doc = timed("get", || editor.get(name, id))?;
        timed("projection", || editor.view_value(name, &doc.value))?;
        timed("draft", || editor.draft(name, &doc.value))?;
        timed("parents cold", || editor.parent_links(name, id))?;
        timed("parents warm", || editor.parent_links(name, id))?;
        for child in editor.config().collection(name)?.child_collections() {
            let request = ListRequest {
                ids: Some(editor.child_ids(name, id, &child.field)?),
                limit: 25,
                ..Default::default()
            };
            timed(&format!("children {}", child.field), || {
                editor.query_collection(&child.collection, &request)
            })?;
        }
        timed("references cold", || editor.cached_references_to(name, id))?;
        timed("references warm", || editor.cached_references_to(name, id))?;
    } else {
        let request = ListRequest {
            limit: 25,
            fields: Some(
                editor.config().collection(name)?.ui["list"]["columns"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect(),
            ),
            ..Default::default()
        };
        for pass in ["list cold", "list warm", "list warm again"] {
            let page = timed(pass, || editor.query_collection(name, &request))?;
            eprintln!("{} records, {} rows", page.total, page.rows.len());
        }
    }
    Ok(())
}
