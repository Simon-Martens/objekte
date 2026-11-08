use super::*;

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
