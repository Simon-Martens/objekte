#[test]
fn relation_work_limits_return_explicit_errors() {
    let mut work = super::RelationWork {
        edge_visits: super::MAX_QUERY_EDGE_VISITS,
        ..Default::default()
    };
    assert!(work.edge().unwrap_err().contains("edge visits"));
    let mut work = super::RelationWork {
        backlink_probe_visits: super::MAX_QUERY_BACKLINK_PROBES,
        ..Default::default()
    };
    assert!(
        work.backlink_probe()
            .unwrap_err()
            .contains("backlink probes")
    );
    let mut work = super::RelationWork {
        target_reads: super::MAX_QUERY_RECORD_READS,
        ..Default::default()
    };
    assert!(work.target_read(1).unwrap_err().contains("target reads"));
    let mut work = super::RelationWork {
        bytes_read: super::MAX_QUERY_RELATION_BYTES,
        ..Default::default()
    };
    assert!(work.source_read(1).unwrap_err().contains("source bytes"));
    let mut counters = crate::measurement::WorkCounters {
        relation_edge_visits: super::MAX_QUERY_EDGE_VISITS + 1,
        ..Default::default()
    };
    assert!(super::check_query_work_budget(&counters).is_err());
    counters.relation_edge_visits = 0;
    counters.relation_bytes_read = super::MAX_QUERY_RELATION_BYTES + 1;
    assert!(super::check_query_work_budget(&counters).is_err());
}
