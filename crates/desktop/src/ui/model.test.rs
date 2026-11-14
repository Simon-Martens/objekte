use super::*;

#[test]
fn open_draft_maps_storage_and_residency_to_core_policy() {
    let mut draft = OpenDraft::default();
    let memory = draft.storage_policy().unwrap();
    assert_eq!(memory.location, IndexStorageLocation::MemoryOnly);
    assert_eq!(memory.residency, IndexResidency::FullyResident);

    draft.index_storage = StorageChoice::Temporary;
    assert_eq!(draft.storage_policy().unwrap().residency, IndexResidency::OnDemand);
    draft.index_storage = StorageChoice::Repository;
    assert_eq!(draft.storage_policy().unwrap().location, IndexStorageLocation::RepositoryDirectory);

    draft.index_storage = StorageChoice::Custom;
    assert!(draft.storage_policy().unwrap_err().contains("base folder"));
    draft.index_directory = "/tmp/index base".into();
    draft.keep_entire_index_in_memory = true;
    let custom = draft.storage_policy().unwrap();
    assert_eq!(custom.location, IndexStorageLocation::CustomDirectory("/tmp/index base".into()));
    assert_eq!(custom.residency, IndexResidency::FullyResident);
}

#[test]
fn focused_storage_base_requires_existing_absolute_directory() {
    let base = tempfile::tempdir().unwrap();
    assert_eq!(canonical_storage_base(base.path().to_str().unwrap()).unwrap(),
        base.path().canonicalize().unwrap());
    assert!(canonical_storage_base("").is_err());
    assert!(canonical_storage_base("relative/base").unwrap_err().contains("absolute"));
    assert!(canonical_storage_base("/this/path/does/not/exist").is_err());
    let file = base.path().join("file");
    std::fs::write(&file, "x").unwrap();
    assert!(canonical_storage_base(file.to_str().unwrap()).unwrap_err().contains("not a directory"));
    let mut current = OpenDraft::default();
    current.index_storage = StorageChoice::Custom;
    current.index_directory = base.path().join(".").display().to_string();
    let mut selected = current.clone();
    selected.index_directory = base.path().canonicalize().unwrap().display().to_string();
    assert!(same_storage_setting(&current, &selected));
}

#[test]
fn page_bounds_rows_columns_and_multibyte_cells_without_changing_total() {
    let rows = (0..MAX_PAGE_ROWS + 3)
        .map(|n| ResultRow {
            key: RowKey {
                token: n as u64,
                collection: "docs".into(),
                record_id: n.to_string(),
                file: "docs/a.json".into(),
                byte_start: n as u64,
            },
            cells: vec!["Ä".repeat(MAX_CELL_BYTES)],
            matched_fields: Vec::new(),
            match_highlights: Vec::new(),
            preview: None,
        })
        .collect();
    let page = PageView {
        columns: vec!["title".into()],
        rows,
        exact_total: Some(1_000_000),
        ..PageView::default()
    }
    .bounded();
    assert_eq!(page.rows.len(), MAX_PAGE_ROWS);
    assert_eq!(page.exact_total, Some(1_000_000));
    assert!(page.has_next);
    assert!(page.clipped);
    assert!(page.rows[0].cells[0].ends_with('…'));
    assert!(page.rows[0].cells[0].is_char_boundary(page.rows[0].cells[0].len()));
    assert_eq!(page.rows[0].key.token, 0);
    assert_eq!(
        page.rows
            .iter()
            .map(|row| row.key.token)
            .collect::<Vec<_>>(),
        (0..MAX_PAGE_ROWS as u64).collect::<Vec<_>>()
    );
}

#[test]
fn page_budget_does_not_change_row_identity() {
    let page = PageView {
        columns: vec!["column".into()],
        rows: (0..10)
            .map(|index| ResultRow {
                key: RowKey {
                    token: index as u64 + 1000,
                    collection: "ä".repeat(MAX_CELL_BYTES),
                    record_id: "id".into(),
                    file: "file".into(),
                    byte_start: 0,
                },
                cells: (0..MAX_COLUMNS)
                    .map(|_| "x".repeat(MAX_CELL_BYTES))
                    .collect(),
                matched_fields: Vec::new(),
                match_highlights: Vec::new(),
                preview: None,
            })
            .collect(),
        ..PageView::default()
    }
    .bounded();
    let displayed_bytes: usize = page.columns.iter().map(String::len).sum::<usize>()
        + page
            .rows
            .iter()
            .map(|row| {
                row.key.collection.len()
                    + row.key.record_id.len()
                    + row.key.file.len()
                    + row.cells.iter().map(String::len).sum::<usize>()
            })
            .sum::<usize>();
    assert!(displayed_bytes <= MAX_PAGE_BYTES);
    assert_eq!(page.rows[0].key.token, 1000);
    assert_eq!(page.rows[9].key.token, 1009);
    assert!(page.clipped);
}

#[test]
fn detail_bounds_raw_source_and_nested_rows() {
    let detail = DetailView {
        structured: (0..MAX_DETAIL_ROWS + 1)
            .map(|n| DetailRow {
                label: n.to_string(),
                value: "value".into(),
                depth: 0,
                matched: false,
                filter_participant: false,
                fts_matched: false,
                value_highlights: Vec::new(),
                expandable: false,
                expanded: false,
                origin: DetailOrigin::Structured,
                value_kind: DetailValueKind::Present,
            })
            .collect(),
        raw_source: "x".repeat(MAX_SOURCE_BYTES + 1),
        ..DetailView::default()
    }
    .bounded();
    assert_eq!(detail.structured.len(), MAX_DETAIL_ROWS);
    assert!(detail.raw_source.ends_with('…'));
    assert!(detail.clipped);
}
