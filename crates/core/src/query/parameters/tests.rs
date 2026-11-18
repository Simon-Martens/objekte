use super::*;

#[test]
fn typed_json_transport_keeps_scalar_types_distinct() {
    assert_eq!(
        parse_parameter_json(r#""a && $b 'quoted'""#).unwrap(),
        Value::String("a && $b 'quoted'".into())
    );
    assert_eq!(
        parse_parameter_json("42").unwrap(),
        Value::Number(Number::integer(42))
    );
    assert_eq!(parse_parameter_json("true").unwrap(), Value::Bool(true));
    assert_eq!(parse_parameter_json("null").unwrap(), Value::Null);
    assert!(matches!(
        parse_parameter_json(r#"{"type":"date","value":"2026-11-06"}"#).unwrap(),
        Value::Date(_)
    ));
    assert!(matches!(
        parse_parameter_json(r#"{"type":"datetime","value":"2026-11-06T14:30:00Z"}"#).unwrap(),
        Value::Datetime(_)
    ));
    assert_eq!(
        parse_parameter_json(r#"{"type":"unixtime","value":-1}"#).unwrap(),
        Value::Unixtime(-1)
    );
    assert!(parse_parameter_json(r#"{"type":"unixtime","value":18446744073709551615}"#).is_err());
    assert!(parse_parameter_json(r#"{"type":"date","value":"2026-02-30"}"#).is_err());
    assert_eq!(
        parse_parameter_json(r#"{"type":"id","value":"x"}"#).unwrap(),
        Value::Id("x".into())
    );
    assert!(parse_parameter_json(r#"{"type":"id","value":""}"#).is_err());
    assert!(parse_parameter_json(r#"{"type":"id","value":1}"#).is_err());
    assert!(parse_parameter_json(r#"{"type":"date","value":"2026-11-06","extra":1}"#).is_err());
}
