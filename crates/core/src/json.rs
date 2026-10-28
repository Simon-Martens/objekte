//! Sonic JSON parsing with the existing public/schema value representation.
//! Keep fingerprint serialization separate: its exact bytes are a cache contract.
use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeSeed, MapAccess, Visitor},
};
use serde_json::{Map, Value};
use sonic_rs::{JsonValueTrait, ValueRef};
use std::{collections::BTreeSet, fmt};

pub(crate) struct ParsedValue(pub Value);

impl<'de> Deserialize<'de> for ParsedValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = sonic_rs::Value::deserialize(deserializer)?;
        compatible(&value, 0).map(Self).map_err(de::Error::custom)
    }
}

// Sonic's DOM preserves duplicate object members. Insert in source order to retain
// the library's last-member-wins contract. Convert raw numbers without an f64 hop.
fn compatible(value: &sonic_rs::Value, depth: usize) -> Result<Value, String> {
    // as_ref() attempts a finite numeric conversion; inspect raw numbers first
    // so opaque large integers and exponents never become rounded values or null.
    if let Some(number) = value.as_raw_number() {
        return number
            .as_str()
            .parse()
            .map(Value::Number)
            .map_err(|e: serde_json::Error| e.to_string());
    }
    match value.as_ref() {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Bool(v) => Ok(Value::Bool(v)),
        ValueRef::String(v) => Ok(Value::String(v.to_owned())),
        ValueRef::Number(_) => unreachable!("numbers handled without a floating-point conversion"),
        ValueRef::Array(values) => {
            if depth >= 127 {
                return Err("recursion limit exceeded".into());
            }
            values
                .iter()
                .map(|v| compatible(v, depth + 1))
                .collect::<Result<_, _>>()
                .map(Value::Array)
        }
        ValueRef::Object(values) => {
            if depth >= 127 {
                return Err("recursion limit exceeded".into());
            }
            let mut object = Map::new();
            for (key, value) in values {
                object.insert(key.to_owned(), compatible(value, depth + 1)?);
            }
            Ok(Value::Object(object))
        }
    }
}

pub(crate) fn parse(json: &str) -> Result<Value, sonic_rs::Error> {
    sonic_rs::from_str::<ParsedValue>(json).map(|v| v.0)
}

/// Materialize requested top-level fields only. Scan all members, including
/// duplicates, using Sonic's native skip behavior for unselected values.
pub(crate) fn project(json: &str, fields: &BTreeSet<&str>) -> Result<Value, sonic_rs::Error> {
    struct Projection<'a>(&'a BTreeSet<&'a str>);
    impl<'de> DeserializeSeed<'de> for Projection<'_> {
        type Value = Value;
        fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<Value, D::Error> {
            de.deserialize_map(self)
        }
    }
    impl<'de> Visitor<'de> for Projection<'_> {
        type Value = Value;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a record object")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Value, M::Error> {
            let mut object = Map::new();
            while let Some(key) = map.next_key::<String>()? {
                if self.0.contains(key.as_str()) {
                    let ParsedValue(value) = map.next_value()?;
                    object.insert(key, value);
                } else {
                    let _: sonic_rs::LazyValue<'de> = map.next_value()?;
                }
            }
            Ok(Value::Object(object))
        }
    }
    let mut de = sonic_rs::Deserializer::from_str(json).use_rawnumber();
    let value = Projection(fields).deserialize(&mut de)?;
    de.end()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_values_preserve_numbers_and_existing_serialization() {
        for number in [
            "0",
            "-0",
            "1.0",
            "-0.0",
            "1.2300",
            "1e3",
            "1E+03",
            "1e-500",
            "1e500",
            "9007199254740993",
            "-9223372036854775809",
            "18446744073709551616",
            "0.123456789012345678901234567890123456789",
        ] {
            let input = format!(r#"{{"number":{number},"array":[{number},null,true,false]}}"#);
            let expected: Value = serde_json::from_str(&input).unwrap();
            let actual = parse(&input).unwrap();
            assert_eq!(actual, expected, "{number}");
            assert_eq!(actual.to_string(), expected.to_string(), "{number}");
        }
    }

    #[test]
    fn duplicate_and_escaped_keys_keep_the_last_value_at_every_level() {
        let input = r#"{"title":"first","t\u0069tle":"last","payload":{"a":1,"a":2},"array":[{"x":0,"x":3}],"quote\"\\":"é 🐈\n\ud83d\ude00"}"#;
        let expected: Value = serde_json::from_str(input).unwrap();
        assert_eq!(parse(input).unwrap(), expected);
        let projected = project(input, &BTreeSet::from(["title", "payload", "quote\"\\"])).unwrap();
        assert_eq!(projected["title"], "last");
        assert_eq!(projected["payload"], expected["payload"]);
        assert_eq!(projected["quote\"\\"], expected["quote\"\\"]);
        assert!(projected.get("array").is_none());
    }

    #[test]
    fn projected_values_match_full_parse_with_null_missing_and_large_payloads() {
        let input = format!(
            r#"{{"ignored":{{"large":"{}","values":[1e500,{{"a":1}}]}},"title":null,"flag":true,"score":9007199254740993,"title":"last"}}"#,
            "x".repeat(128 * 1024)
        );
        let full: Value = serde_json::from_str(&input).unwrap();
        let fields = BTreeSet::from(["title", "flag", "score", "missing"]);
        let expected: Map<_, _> = full
            .as_object()
            .unwrap()
            .iter()
            .filter(|(key, _)| fields.contains(key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        assert_eq!(project(&input, &fields).unwrap(), Value::Object(expected));
        assert_eq!(
            project(r#"{"title":null}"#, &fields).unwrap()["title"],
            Value::Null
        );
        assert!(project("{}", &fields).unwrap().get("title").is_none());
    }

    #[test]
    fn malformed_json_is_rejected_even_in_unselected_fields() {
        for input in [
            r#"{"title":"ok","ignored":[1,]}"#,
            r#"{"title":"ok","ignored":01}"#,
            r#"{"title":"ok","ignored":"\x"}"#,
            r#"{"title":"ok","ignored":true false}"#,
            r#"{"title":"ok"} true"#,
            r#"{"title":"ok","ignored":{"a":1,}}"#,
        ] {
            assert!(parse(input).is_err(), "full: {input}");
            assert!(
                project(input, &BTreeSet::from(["title"])).is_err(),
                "projected: {input}"
            );
        }
        for input in ["null", "[]", "1", r#""text""#] {
            assert!(project(input, &BTreeSet::new()).is_err());
        }
        assert!(sonic_rs::from_slice::<ParsedValue>(b"{\"bad\":\"\xff\"}").is_err());
    }

    #[test]
    fn projection_does_not_reparse_skipped_surrogate_escapes() {
        let input = r#"{"ignored":{"values":["\ud800","\udc00","\ud83d\ude00"]},"title":"ok"}"#;
        assert_eq!(
            project(input, &BTreeSet::from(["title"])).unwrap(),
            serde_json::json!({"title": "ok"})
        );
    }

    #[test]
    fn full_parse_keeps_the_existing_nesting_limit() {
        // Sonic's own recursion-limit test uses a 16 MiB stack: its unoptimized
        // DOM decoder needs more than Rust's default 2 MiB test-thread stack.
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                for depth in [1, 126, 127, 128, 200] {
                    let input = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
                    assert_eq!(
                        parse(&input).is_ok(),
                        serde_json::from_str::<Value>(&input).is_ok(),
                        "depth {depth}"
                    );
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
