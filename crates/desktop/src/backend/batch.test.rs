use super::*;
use crate::backend::test_support::fixture;
use objekte_cli::execution::run;
use serde_json::Value;

fn events(bytes: &[u8]) -> Vec<Value> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect()
}

fn stable(mut events: Vec<Value>) -> Vec<Value> {
    for event in &mut events {
        let object = event.as_object_mut().unwrap();
        object.retain(|key, _| {
            !key.ends_with("_ms") && !matches!(key.as_str(), "build_parses" | "build_failed_parses")
        });
        if let Some(counters) = object.get_mut("counters").and_then(Value::as_object_mut) {
            counters.retain(|key, _| !key.ends_with("_ms"));
        }
    }
    events
}

#[test]
fn retained_batch_matches_cli_protocol_and_continues_after_query_error() {
    let fixture = fixture();
    let output = tempfile::tempdir().unwrap();
    let input_path = output.path().join("queries.txt");
    std::fs::write(
        &input_path,
        "\u{feff}year >= 2023\n\nmissing ???\n\"Alpha\"\n",
    )
    .unwrap();
    let mut args = fixture.session.args.clone();
    args.query_file = Some(input_path.to_string_lossy().into_owned());
    let mut direct = Vec::new();
    let direct_exit = run(&args, &mut direct, &CancellationToken::new());
    let path = output.path().join("batch.ndjson");
    let report = run_batch(
        &fixture.session,
        &BatchSpec {
            source: BatchSource::File(input_path),
            offset: 0,
            limit: None,
            count: false,
        },
        &path,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(report.exit, direct_exit);
    let actual = std::fs::read(path).unwrap();
    let types: Vec<_> = events(&actual)
        .iter()
        .map(|event| event["type"].as_str().unwrap().to_owned())
        .collect();
    assert!(types.contains(&"query_error".to_owned()));
    assert_eq!(types.last().unwrap(), "run_error");
    assert_eq!(stable(events(&actual)), stable(events(&direct)));
}

#[test]
fn output_rejects_source_directory_and_existing_file() {
    let fixture = fixture();
    let root = fixture._root.path();
    let spec = BatchSpec {
        source: BatchSource::Text("year >= 2023".into()),
        offset: 0,
        limit: Some(2),
        count: true,
    };
    let inside = root.join("batch.ndjson");
    let error = run_batch(&fixture.session, &spec, &inside, &CancellationToken::new()).unwrap_err();
    assert_eq!(error.kind, BackendErrorKind::Options);
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("batch.ndjson");
    std::fs::write(&path, "leave this alone").unwrap();
    let error = run_batch(&fixture.session, &spec, &path, &CancellationToken::new()).unwrap_err();
    assert_eq!(error.kind, BackendErrorKind::Output);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "leave this alone");
}

#[test]
fn missing_query_file_emits_cli_input_error_after_run_start() {
    let fixture = fixture();
    let output = tempfile::tempdir().unwrap();
    let report = run_batch(
        &fixture.session,
        &BatchSpec {
            source: BatchSource::File(output.path().join("missing.txt")),
            offset: 0,
            limit: None,
            count: false,
        },
        &output.path().join("batch.ndjson"),
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(report.exit, Exit::Operation);
    let output = std::fs::read(report.path).unwrap();
    let events = events(&output);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "run_start");
    assert_eq!(events[1]["type"], "run_error");
    assert_eq!(events[1]["stage"], "query_input");
}

#[test]
fn direct_structured_and_full_text_match_cli_protocol() {
    let fixture = fixture();
    let output = tempfile::tempdir().unwrap();
    for (name, source) in [
        (
            "structured",
            BatchSource::DirectStructured("year >= 2023".into()),
        ),
        ("fts", BatchSource::DirectFullText("Alpha".into())),
    ] {
        let mut args = fixture.session.args.clone();
        match &source {
            BatchSource::DirectStructured(text) => args.query = Some(text.clone()),
            BatchSource::DirectFullText(text) => args.fts = Some(text.clone()),
            _ => unreachable!(),
        }
        let mut direct = Vec::new();
        let direct_exit = run(&args, &mut direct, &CancellationToken::new());
        let path = output.path().join(format!("{name}.ndjson"));
        let report = run_batch(
            &fixture.session,
            &BatchSpec {
                source,
                offset: 0,
                limit: None,
                count: false,
            },
            &path,
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(report.exit, direct_exit);
        assert_eq!(
            stable(events(&std::fs::read(path).unwrap())),
            stable(events(&direct))
        );
    }
}
