use super::*;
use crate::JsonPathStep;

#[test]
fn built_in_source_and_format_identities_cannot_be_replaced() {
    struct ReservedProvider(Descriptor);

    impl SourceProvider for ReservedProvider {
        fn descriptor(&self) -> &Descriptor {
            &self.0
        }

        fn enumerate(&self, _: &str) -> Result<Vec<SourceDescriptor>> {
            Ok(Vec::new())
        }

        fn read(&self, _: &SourceDescriptor, _: &mut Vec<u8>) -> Result<()> {
            Ok(())
        }
    }

    let mut registry = Extensions::default();
    let provider = ReservedProvider(Descriptor::new(
        "objekte:filesystem",
        SemanticVersion { major: 1, minor: 0, patch: 0 },
        &(),
        DataKind::Source,
        DataKind::Source,
        vec![],
    ));
    assert!(matches!(registry.register_source(provider), Err(Error::Request(message)) if message.contains("reserved")));
    assert!(matches!(registry.register_format(JsonFormatDecoder::default()), Err(Error::Request(message)) if message.contains("reserved")));
}

#[test]
fn native_json_decoder_visits_pointer_paths_and_propagates_callback_errors() {
    let decoder = JsonFormatDecoder::default();
    let empty = vec![JsonPathStep::Key("empty".into())];
    decoder
        .decode(br#"{"empty":[]}"#, &mut |record| {
            assert!(record.path_present(&empty)?);
            assert_eq!(record.visit_path(&empty, &mut |_| Ok(()))?, 0);
            Ok(())
        })
        .unwrap();
    let mut leaves = Vec::new();
    decoder
        .visit_leaves(br#"{"a/b":[1,null],"~":true}"#, &mut |path, value| {
            leaves.push((
                path.to_owned(),
                match value {
                    ScalarValue::Null => "null".into(),
                    ScalarValue::Bool(value) => value.to_string(),
                    ScalarValue::Number(value) | ScalarValue::Text(value) => value.into(),
                },
            ));
            Ok(())
        })
        .unwrap();
    assert_eq!(
        leaves,
        [
            ("/a~1b/0".into(), "1".into()),
            ("/a~1b/1".into(), "null".into()),
            ("/~0".into(), "true".into())
        ]
    );
    let mut called = 0;
    let error = decoder.visit_leaves(br#"[1,2]"#, &mut |_, _| {
        called += 1;
        Err(Error::Request("stopped".into()))
    });
    assert_eq!(called, 1);
    assert!(matches!(error, Err(Error::Request(message)) if message == "stopped"));
    assert!(decoder.visit_leaves(b"{", &mut |_, _| Ok(())).is_err());
    let mut duplicate = Vec::new();
    decoder
        .visit_leaves(br#"{"title":"old","title":"new"}"#, &mut |_, value| {
            if let ScalarValue::Text(value) = value {
                duplicate.push(value.to_owned());
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(duplicate, ["new"]);
}

#[test]
fn borrowed_record_access_preserves_presence_order_and_number_spelling() {
    let source = r#"{"dates":["circa 1848",null,1.0,1e100],"empty":[]}"#;
    let json: Value = serde_json::from_str(source).unwrap();
    let sonic: SonicValue = sonic_rs::from_str(source).unwrap();
    let dates = vec![JsonPathStep::Key("dates".into())];
    let empty = vec![JsonPathStep::Key("empty".into())];
    let missing = vec![JsonPathStep::Key("missing".into())];
    for record in [&json as &dyn RecordAccess, &sonic as &dyn RecordAccess] {
        assert!(record.path_present(&dates).unwrap());
        assert!(record.path_present(&empty).unwrap());
        assert!(!record.path_present(&missing).unwrap());
        assert_eq!(record.visit_path(&empty, &mut |_| Ok(())).unwrap(), 0);
        let mut values = Vec::new();
        assert_eq!(
            record
                .visit_path(&dates, &mut |value| {
                    values.push(match value {
                        ScalarValue::Null => "null".into(),
                        ScalarValue::Text(text) | ScalarValue::Number(text) => text.into(),
                        ScalarValue::Bool(value) => value.to_string(),
                    });
                    Ok(())
                })
                .unwrap(),
            4
        );
        assert_eq!(values, ["circa 1848", "null", "1.0", "1e+100"]);
    }
}

#[test]
fn borrowed_record_callback_failure_stops_iteration() {
    let source = serde_json::json!({"values":[1,2,3]});
    let path = vec![JsonPathStep::Key("values".into())];
    let mut seen = 0;
    let error = source.visit_path(&path, &mut |_| {
        seen += 1;
        Err(Error::Request("stopped".into()))
    });
    assert_eq!(seen, 1);
    assert!(matches!(error, Err(Error::Request(message)) if message == "stopped"));
}
