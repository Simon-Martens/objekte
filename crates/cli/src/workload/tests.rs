use super::*;

#[test]
fn fixed_manifest_and_report_round_trip() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
    let cases = load_manifest(&path).unwrap();
    assert_eq!(cases.len(), 300);
    validate_manifest(&cases[..150]).unwrap();
    assert_eq!(cases[0].fts.as_deref(), Some("Almanach"));
    assert_eq!(
        cases[69].filter.as_deref(),
        Some("name:folded ~ \"GOETHE\"")
    );
    assert_eq!(cases[149].sort[0].field, "preferred_title");
    assert_eq!(
        cases,
        serde_json::from_slice::<Vec<WorkloadCase>>(&serde_json::to_vec(&cases).unwrap()).unwrap()
    );
    let sample = TimingSample {
        case_id: "Q001".into(),
        profile: PROFILES[0],
        repetition: 0,
        status: "pending".into(),
        error: None,
        total_ns: None,
        counters: None,
    };
    let serialized = serde_json::to_vec(&sample).unwrap();
    let decoded: TimingSample = serde_json::from_slice(&serialized).unwrap();
    assert_eq!(decoded.case_id, sample.case_id);
    assert_eq!(decoded.total_ns, None);
    assert_eq!(PROFILES.iter().filter(|p| p.warm).count(), 2);
}

#[test]
fn extended_manifest_preserves_the_fixed_150_and_binds_parameters() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches");
    let fixed = directory.join("musenalm_queries.json");
    let extended = directory.join("musenalm_queries_extended.json");
    let cases = load_extended_manifest(&fixed, &extended).unwrap();
    assert_eq!(cases.len(), 370);
    assert_eq!(cases[220].id, "Q221");
    assert_eq!(cases[369].id, "Q370");
    assert_eq!(cases[0].request, load_manifest(&fixed).unwrap()[0]);
    assert!(cases[..150].iter().all(|case| case.parameters.is_empty()));
    assert_eq!(
        cases[150].filter.as_deref(),
        Some("SORT BY year DESC, preferred_title ASC")
    );
    assert!(cases[156].query().is_ok());
    assert!(cases[182].query().is_ok());
    assert!(cases[183].query().is_ok());
    assert!(cases[187].query().is_ok());
    assert!(cases[188..].iter().all(|case| case.query().is_ok()));
    let mut changed = cases.clone();
    changed[0].request.fts = Some("changed".into());
    let scratch = std::env::temp_dir().join(format!("objekte-workload-{}", std::process::id()));
    std::fs::write(&scratch, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(load_extended_manifest(&fixed, &scratch).is_err());
    std::fs::remove_file(scratch).unwrap();
}

#[test]
fn every_fixed_filter_parses() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
    for case in load_manifest(&path).unwrap() {
        query_from_workload(&case).unwrap_or_else(|error| panic!("{}: {error}", case.id));
    }
}

#[test]
fn historical_workload_omits_default_fts_options() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
    let cases = load_manifest(&path).unwrap();
    let encoded = serde_json::to_value(&cases[0]).unwrap();
    assert!(encoded.get("fts_mode").is_none());
    assert!(encoded.get("fts_case_sensitivity").is_none());
}
