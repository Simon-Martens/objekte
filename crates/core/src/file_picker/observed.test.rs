use super::*;


fn observation(path: &[&str], kind: ObservedKind, scalar: Option<TypedKey>) -> FieldObservation {
    FieldObservation {
        path: path.iter().map(|part| (*part).to_owned()).collect(),
        kinds: kind.bit(),
        occurrences: 1,
        scalar,
    }
}

#[test]
fn tracks_nested_shape_missing_null_and_refresh() {
    let mut observed = ObservedFields::new(3, false, &[]);
    observed.add(
        0,
        &[
            observation(&["person"], ObservedKind::Object, None),
            observation(
                &["person", "age"],
                ObservedKind::Number,
                Some(TypedKey::Number("30".into())),
            ),
        ],
    );
    observed.add(
        1,
        &[
            observation(&["person"], ObservedKind::Object, None),
            observation(&["person", "age"], ObservedKind::Null, Some(TypedKey::Null)),
        ],
    );
    observed.add(2, &[observation(&["person"], ObservedKind::Array, None)]);
    let age = observed
        .profiles()
        .into_iter()
        .find(|p| p.path == ["person", "age"])
        .unwrap();
    assert_eq!(
        (age.present, age.missing, age.nulls, age.numbers),
        (2, 1, 1, 1)
    );
    assert_eq!(
        observed.missing_bits(&["person".into(), "age".into()]),
        [0b100]
    );

    observed.update(
        1,
        Some(&[observation(
            &["person", "age"],
            ObservedKind::Boolean,
            Some(TypedKey::Bool(true)),
        )]),
    );
    let age = observed
        .profiles()
        .into_iter()
        .find(|p| p.path == ["person", "age"])
        .unwrap();
    assert_eq!((age.nulls, age.booleans, age.numbers), (0, 1, 1));
    observed.update(0, None);
    let age = observed
        .profiles()
        .into_iter()
        .find(|p| p.path == ["person", "age"])
        .unwrap();
    assert_eq!((age.present, age.missing), (1, 1));
}

#[test]
fn drops_diverse_strings_and_multi_value_paths_from_inference() {
    let mut observed = ObservedFields::new(67, true, &[]);
    for slot in 0..65 {
        observed.add(
            slot,
            &[
                observation(
                    &["kind"],
                    ObservedKind::String,
                    Some(TypedKey::String(format!("v{slot}"))),
                ),
                observation(
                    &["flag"],
                    ObservedKind::Boolean,
                    Some(TypedKey::Bool(slot % 2 == 0)),
                ),
            ],
        );
    }
    let mut repeated = observation(&["nested", "tag"], ObservedKind::String, None);
    repeated.occurrences = 2;
    observed.add(65, &[repeated]);
    let rows = observed.take_inferred();
    assert!(!rows.contains_key(&vec!["kind".into()]));
    assert!(!rows.contains_key(&vec!["nested".into(), "tag".into()]));
    assert_eq!(rows[&vec!["flag".into()]].len(), 65);
}

#[test]
fn records_mixed_shapes_reached_through_arrays() {
    let mut observed = ObservedFields::new(1, false, &[]);
    let mut reached = observation(&["contributors", "type"], ObservedKind::String, None);
    reached.kinds |= ObservedKind::Null.bit();
    reached.occurrences = 2;
    observed.add(0, &[reached]);
    let field = observed.profiles().into_iter().next().unwrap();
    assert_eq!(
        (
            field.strings,
            field.nulls,
            field.mixed_records,
            field.multiple_values
        ),
        (1, 1, 1, 1)
    );
}
