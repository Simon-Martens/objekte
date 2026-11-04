use super::*;
use crate::JsonPathStep;

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
