use super::*;

fn key(session_generation: u64, instance_id: u64, revision: u64) -> PublicationKey {
    PublicationKey {
        session_generation,
        instance_id,
        revision: IndexRevision(revision),
    }
}

#[test]
fn publication_identity_rejects_replacements_with_equal_numeric_revisions() {
    let old = key(1, 101, 7);
    assert!(old.is_current(old));
    assert!(!old.is_current(key(1, 102, 7)));
    assert!(!old.is_current(key(1, 101, 8)));
    assert!(!old.is_current(key(2, 101, 7)));
}

#[test]
fn result_and_tab_jobs_require_their_own_generations() {
    let publication = key(1, 101, 7);
    let query = ResultContextKey {
        publication,
        query_generation: 3,
    };
    assert!(query.owns_response(query));
    assert!(!query.owns_response(ResultContextKey {
        query_generation: 4,
        ..query
    }));
    let tab = TabJobKey {
        publication,
        tab_id: 11,
        operation_generation: 2,
    };
    assert!(tab.owns_response(tab));
    assert!(!tab.owns_response(TabJobKey { tab_id: 12, ..tab }));
    assert!(!tab.owns_response(TabJobKey {
        operation_generation: 3,
        ..tab
    }));
    assert!(!tab.owns_response(TabJobKey {
        publication: key(1, 101, 8),
        ..tab
    }));
}
