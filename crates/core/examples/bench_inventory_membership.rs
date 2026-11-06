//! Focused synthetic admission benchmark; no timing assertion belongs in tests.

use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::inventory::{CollectionBuilder, FileInventory, SourceRevision};
use objekte_search_core::model::{CollectionId, RecordId};

fn main() -> Result<(), String> {
    for count in [1_000usize, 2_000, 4_000, 8_000, 16_000] {
        let mut inventory = FileInventory::new(PathBuf::from("/synthetic"));
        let revision = SourceRevision {
            modified_ns: 1,
            size: 1,
        };
        let mut ids = Vec::with_capacity(count);
        for index in 0..count {
            let file = inventory.add(
                PathBuf::from(format!("{index:08}.json")),
                revision,
                None,
                CollectionId(0),
            )?;
            ids.push(RecordId::generated(file, revision.modified_ns, 0));
        }
        let mut samples_ns = Vec::new();
        for _ in 0..5 {
            let mut builder = CollectionBuilder::new(CollectionId(0), "data".into(), 1, count);
            let started = Instant::now();
            for (file, id) in inventory.files.iter().zip(&ids) {
                builder.include_file(file.id)?;
                builder.add_record(file, id.clone(), 0, 1)?;
            }
            samples_ns.push(started.elapsed().as_nanos());
            let generation = builder.finish(&inventory)?;
            assert_eq!(generation.records.len(), count);
        }
        println!(
            "{}",
            serde_json::json!({"files": count, "samples_ns": samples_ns})
        );
    }
    Ok(())
}
