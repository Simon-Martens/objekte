use super::*;

#[test]
fn automatic_mode_waits_for_detection_and_preserves_invalid_configuration() {
    let mut dialog = OpenDialogState::default();
    dialog.show();
    dialog.edit_folder(0, "/tmp/repository".into());
    assert!(dialog.to_open_draft(&OpenDraft::default()).is_err());
    let (generation, path) = dialog.begin_detection().unwrap();
    assert!(dialog.accept_detection(
        generation,
        &path,
        Ok(DetectedSource::InvalidConfiguration {
            message: "broken.schema.json: invalid JSON".into(),
        })
    ));
    assert!(
        dialog
            .to_open_draft(&OpenDraft::default())
            .unwrap_err()
            .contains("broken.schema.json")
    );
    dialog.source_choice = SourceChoice::Folder;
    assert_eq!(
        dialog.to_open_draft(&OpenDraft::default()).unwrap().mode,
        OpenMode::Folder
    );
}

#[test]
fn picker_and_detection_reject_late_results_after_edit_or_close() {
    let mut dialog = OpenDialogState::default();
    dialog.show();
    dialog.edit_folder(0, "/tmp/old".into());
    let (generation, path) = dialog.begin_detection().unwrap();
    let picker = dialog.begin_picker(0).unwrap();
    dialog.edit_folder(0, "/tmp/new".into());
    assert!(!dialog.accept_detection(generation, &path, Ok(DetectedSource::PlainFolder)));
    assert!(!dialog.accept_picker(picker, 0, "/tmp/stale".into()));
    dialog.close();
    assert!(!dialog.accept_picker(picker, 0, "/tmp/stale".into()));
    assert_eq!(dialog.folders[0], "/tmp/new");
}

#[test]
fn editing_folder_invalidates_pending_schema_picker() {
    let mut dialog = OpenDialogState::default();
    dialog.show();
    let generation = dialog.begin_schema_picker().unwrap();
    dialog.edit_folder(0, "/tmp/replaced".into());
    assert!(!dialog.accept_schema_picker(generation));
}

#[test]
fn adding_and_removing_rows_switches_to_multiple_folder_request() {
    let mut dialog = OpenDialogState::default();
    dialog.show();
    dialog.edit_folder(0, "/tmp/one".into());
    let (generation, path) = dialog.begin_detection().unwrap();
    dialog.accept_detection(
        generation,
        &path,
        Ok(DetectedSource::ConfiguredRepository {
            collections: 2,
            schema_files: vec!["/tmp/one/.objekte/collections/a.json".into()],
        }),
    );
    let second = dialog.add_folder();
    assert!(dialog.to_open_draft(&OpenDraft::default()).is_err());
    dialog.edit_folder(second, "/tmp/two".into());
    let draft = dialog.to_open_draft(&OpenDraft::default()).unwrap();
    assert_eq!(draft.mode, OpenMode::MultipleFolders);
    assert_eq!(draft.paths, "/tmp/one\n/tmp/two");
    dialog.remove_folder(second);
    assert_eq!(dialog.folders.len(), 1);
    assert!(dialog.to_open_draft(&OpenDraft::default()).is_err());
}

#[test]
fn configured_repository_defaults_to_memory_and_custom_requires_a_base_folder() {
    let mut dialog = OpenDialogState::default();
    dialog.show();
    dialog.edit_folder(0, "/tmp/one".into());
    let (generation, path) = dialog.begin_detection().unwrap();
    dialog.accept_detection(generation, &path, Ok(DetectedSource::ConfiguredRepository {
        collections: 1,
        schema_files: vec!["/tmp/one/.objekte/collections/a.json".into()],
    }));
    let default = dialog.to_open_draft(&OpenDraft::default()).unwrap();
    assert_eq!(default.mode, OpenMode::ConfiguredRepository);
    assert_eq!(default.index_storage, StorageChoice::Memory);
    let custom = OpenDraft {
        index_storage: StorageChoice::Custom,
        ..OpenDraft::default()
    };
    assert!(dialog.to_open_draft(&custom).unwrap_err().contains("base folder"));
    let custom = OpenDraft { index_directory: "/tmp/indexes".into(), ..custom };
    assert_eq!(dialog.to_open_draft(&custom).unwrap().index_directory, "/tmp/indexes");
}

#[test]
fn custom_folder_picker_rejects_late_results() {
    let mut dialog = OpenDialogState::default();
    dialog.show();
    let generation = dialog.begin_index_picker().unwrap();
    assert!(dialog.accept_index_picker(generation));
    let stale = dialog.begin_index_picker().unwrap();
    dialog.close();
    assert!(!dialog.accept_index_picker(stale));
}

#[test]
fn focused_storage_dialog_keeps_source_fixed_and_cancel_resets_mode() {
    let mut dialog = OpenDialogState::default();
    dialog.show_storage();
    assert!(dialog.visible && dialog.storage_only);
    assert!(dialog.begin_index_picker().is_some());
    dialog.close();
    assert!(!dialog.visible && !dialog.storage_only);
    dialog.show();
    assert!(!dialog.storage_only);
}

#[test]
fn schema_picker_is_rejected_after_dialog_closes() {
    let mut dialog = OpenDialogState::default();
    dialog.show();
    let generation = dialog.begin_schema_picker().unwrap();
    dialog.close();
    assert!(!dialog.accept_schema_picker(generation));
}
