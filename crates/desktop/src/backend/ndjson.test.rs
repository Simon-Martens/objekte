use super::*;
use crate::backend::{BackendErrorKind, query_page, test_support::fixture};
use objekte_cli::execution::Exit;
use serde_json::Value;

fn events(path: &Path) -> Vec<Value> {
    std::fs::read(path)
        .unwrap()
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect()
}

#[test]
fn combined_query_exports_full_sorted_range_with_cli_event_order() {
    let fixture = fixture();
    let token = CancellationToken::new();
    let spec = QuerySpec {
        full_text: "alpha".into(),
        filter: "year >= 2022 SORT BY year DESC".into(),
        case_sensitive: false,
        count: true,
        ..Default::default()
    };
    let viewport = query_page(&fixture.session, &spec, 0, 1, &token).unwrap();
    assert_eq!(viewport.hits.len(), 1);
    let expected = query_page(&fixture.session, &spec, 0, 10, &token).unwrap();
    assert_eq!(expected.count, Some(2));
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("combined.ndjson");
    let report = export_query_ndjson(&fixture.session, &spec, &path, &token).unwrap();
    assert_eq!(report.exit, Exit::Success);
    let events = events(&path);
    let kinds: Vec<_> = events
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "run_start",
            "query_start",
            "hit",
            "hit",
            "query_complete",
            "run_complete"
        ]
    );
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event["sequence"], index + 1);
    }
    assert_eq!(events[0]["input_mode"], "combined");
    assert_eq!(events[1]["kind"], "combined");
    assert_eq!(events[1]["scope"]["type"], "all");
    assert_eq!(events[1]["count_requested"], true);
    let input: Value = serde_json::from_str(events[1]["input"].as_str().unwrap()).unwrap();
    assert_eq!(input["full_text"], "alpha");
    assert_eq!(input["filter"], "year >= 2022 SORT BY year DESC");
    assert!(input.get("sort").is_none());
    for (event, hit) in events[2..4].iter().zip(&expected.hits) {
        assert_eq!(event["file"]["record_id"], hit.record_id);
        assert_eq!(event["requested_offset"], 0);
        assert_eq!(event["data_available"], true);
    }
    assert_eq!(events[4]["exact_total"], 2);
    assert_eq!(events[4]["emitted_rows"], 2);
    assert_eq!(events[5]["query_successes"], 1);

    let ranged = QuerySpec {
        offset: 1,
        limit: Some(1),
        ..spec
    };
    let range_path = output.path().join("range.ndjson");
    export_query_ndjson(&fixture.session, &ranged, &range_path, &token).unwrap();
    let range = self::events(&range_path);
    let hits: Vec<_> = range
        .iter()
        .filter(|event| event["type"] == "hit")
        .collect();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["file"]["record_id"], expected.hits[1].record_id);
    assert_eq!(hits[0]["requested_offset"], 1);
    assert_eq!(range[1]["scope"]["limit"], 1);
    assert_eq!(range[3]["exact_total"], 2);
}

#[test]
fn pure_queries_keep_cli_direct_modes_and_requested_range() {
    let fixture = fixture();
    let output = tempfile::tempdir().unwrap();
    for (name, spec, mode, kind) in [
        (
            "structured",
            QuerySpec {
                filter: "year >= 2022".into(),
                offset: 1,
                limit: Some(1),
                count: true,
                ..Default::default()
            },
            "positional",
            "structured",
        ),
        (
            "full_text",
            QuerySpec {
                full_text: "Alpha".into(),
                ..Default::default()
            },
            "fts",
            "fts",
        ),
    ] {
        let path = output.path().join(format!("{name}.ndjson"));
        export_query_ndjson(&fixture.session, &spec, &path, &CancellationToken::new()).unwrap();
        let events = events(&path);
        assert_eq!(events[0]["input_mode"], mode);
        assert_eq!(events[1]["kind"], kind);
        if name == "structured" {
            assert_eq!(events[1]["scope"]["offset"], 1);
            assert_eq!(events[1]["scope"]["limit"], 1);
            assert_eq!(events[3]["exact_total"], 3);
        }
    }
}

#[test]
fn invalid_combination_emits_input_error_and_output_never_overwrites() {
    let fixture = fixture();
    let output = tempfile::tempdir().unwrap();
    let spec = QuerySpec {
        full_text: "alpha".into(),
        filter: "year >= 2022 SORT BY".into(),
        ..Default::default()
    };
    let path = output.path().join("error.ndjson");
    let report =
        export_query_ndjson(&fixture.session, &spec, &path, &CancellationToken::new()).unwrap();
    assert_eq!(report.exit, Exit::Operation);
    let events = events(&path);
    let kinds: Vec<_> = events
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["run_start", "query_start", "query_error", "run_error"]
    );
    assert_eq!(events[2]["stage"], "input");
    let error =
        export_query_ndjson(&fixture.session, &spec, &path, &CancellationToken::new()).unwrap_err();
    assert_eq!(error.kind, BackendErrorKind::Output);
    let inside = fixture._root.path().join("inside.ndjson");
    let error = export_query_ndjson(&fixture.session, &spec, &inside, &CancellationToken::new())
        .unwrap_err();
    assert_eq!(error.kind, BackendErrorKind::Options);
}
