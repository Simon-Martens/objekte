use super::*;

#[test]
fn fixed_manifest_and_report_round_trip() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
    let cases = load_manifest(&path).unwrap();
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
