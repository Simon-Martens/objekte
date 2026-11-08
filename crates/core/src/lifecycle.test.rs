use std::sync::Arc;

use super::{Index, IndexError, IndexRevision, PublishedIndex};
use crate::model::FileId;
use crate::query::{Query, QueryOptions};
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions, SnapshotMode};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

fn fixture_repository(root: &std::path::Path) -> Repository {
    Repository::open_with_options(
        root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap()
}

fn all_records() -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

#[test]
fn pinned_view_stays_stable_when_a_new_root_is_published() {
    let root = std::env::temp_dir().join(format!(
        "objekte-published-view-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), b"{\"name\":\"A\"}").unwrap();
    let index = Index::from_repository(fixture_repository(&root)).unwrap();
    let clone = index.clone();
    let old = index.view().unwrap();
    let first_file = old.file_handle(FileId(0)).unwrap();
    assert_eq!(old.revision(), IndexRevision::INITIAL);
    assert_eq!(index.status().unwrap().revision, IndexRevision::INITIAL);
    assert_eq!(old.search(&all_records()).unwrap().count, Some(1));

    std::fs::write(root.join("data/b.json"), b"{\"name\":\"B\"}").unwrap();
    let new_revision = old.revision().next().unwrap();
    let prepared = PublishedIndex::from_repository(
        index.instance_id(),
        new_revision,
        Arc::new(fixture_repository(&root)),
    )
    .unwrap();
    assert_eq!(index.status().unwrap().revision, IndexRevision::INITIAL);
    assert_eq!(
        index.publish_prepared(old.revision(), prepared).unwrap(),
        new_revision
    );
    let current = clone.view().unwrap();
    assert_eq!(current.revision(), new_revision);
    assert_eq!(current.search(&all_records()).unwrap().count, Some(2));
    assert_eq!(current.file_version(FileId(0)).unwrap().0, new_revision.0);
    assert_eq!(old.file_handle(FileId(0)).unwrap(), first_file);
    assert_eq!(old.search(&all_records()).unwrap().count, Some(1));
    assert_eq!(
        old.active_slots(crate::model::CollectionId(0))
            .unwrap()
            .len(),
        1
    );
    assert!(old.is_active(crate::model::CollectionId(0), crate::model::RecordSlot(0)));

    let stale = PublishedIndex::from_repository(
        index.instance_id(),
        new_revision,
        Arc::clone(&current.0.repository),
    )
    .unwrap();
    assert!(matches!(
        index.publish_prepared(old.revision(), stale),
        Err(IndexError::RevisionConflict { expected, actual })
            if expected == IndexRevision::INITIAL && actual == new_revision
    ));
    assert_eq!(index.status().unwrap().revision, new_revision);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn publication_rejects_foreign_and_skipped_revisions_without_advancing_status() {
    let root = std::env::temp_dir().join(format!("objekte-pub-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), b"{\"name\":\"A\"}").unwrap();
    let index = Index::from_repository(fixture_repository(&root)).unwrap();
    let initial = index.status().unwrap();
    let foreign = PublishedIndex::from_repository(
        index.instance_id() + 1,
        IndexRevision(2),
        Arc::new(fixture_repository(&root)),
    )
    .unwrap();
    assert!(matches!(
        index.publish_prepared(IndexRevision::INITIAL, foreign),
        Err(IndexError::ForeignInstance)
    ));
    let skipped = PublishedIndex::from_repository(
        index.instance_id(),
        IndexRevision(3),
        Arc::new(fixture_repository(&root)),
    )
    .unwrap();
    assert!(matches!(
        index.publish_prepared(IndexRevision::INITIAL, skipped),
        Err(IndexError::InvalidPublication(_))
    ));
    assert_eq!(index.status().unwrap(), initial);
    std::fs::remove_dir_all(root).unwrap();
}
