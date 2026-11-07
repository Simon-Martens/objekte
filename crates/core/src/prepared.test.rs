use super::*;
use std::sync::{Arc, Barrier, Mutex};

fn empty_query() -> PreparedQuery {
    PreparedQuery {
        collections: Vec::new(),
    }
}

fn insert(cache: &mut PreparedCache, key: &[u8], query: PreparedQuery) {
    let bytes = PreparedCache::entry_bytes(key, &query);
    cache.insert(key.to_vec(), query, bytes);
}

fn assert_cache_accounting(cache: &PreparedCache) {
    assert_eq!(cache.order.len(), cache.entries.len());
    assert_eq!(
        cache.bytes,
        cache
            .entries
            .values()
            .map(|entry| entry.bytes)
            .sum::<usize>()
    );
    for key in &cache.order {
        assert!(cache.entries.contains_key(key));
    }
}

#[test]
fn bounded_lru_evicts_without_exceeding_budget() {
    let query = empty_query();
    let one_charge = PreparedCache::entry_bytes(b"a", &query);
    let mut cache = PreparedCache::new(one_charge * 2);
    insert(&mut cache, b"a", query.clone());
    assert!(cache.get(b"a").is_some());
    insert(&mut cache, b"b", query.clone());
    assert!(cache.used_bytes() <= one_charge * 2);
    assert!(cache.get(b"a").is_some());
    insert(&mut cache, b"c", query);
    assert!(cache.get(b"b").is_none());
    assert!(cache.get(b"a").is_some());
    assert!(cache.get(b"c").is_some());
}

#[test]
fn repeated_insert_keeps_one_entry_and_one_charge() {
    let mut cache = PreparedCache::new(1024);
    insert(&mut cache, b"same", empty_query());
    let first_charge = cache.used_bytes();

    insert(&mut cache, b"same", empty_query());

    assert_eq!(cache.entries.len(), 1);
    assert_eq!(cache.used_bytes(), first_charge);
    assert_eq!(cache.order, VecDeque::from([b"same".to_vec()]));
    assert_cache_accounting(&cache);
}

#[test]
fn replacement_updates_charge_when_preparation_size_changes() {
    let mut cache = PreparedCache::new(4096);
    insert(&mut cache, b"same", empty_query());
    let first_charge = cache.used_bytes();
    insert(
        &mut cache,
        b"same",
        PreparedQuery {
            collections: vec![(0, None, None), (1, None, None)],
        },
    );

    assert!(cache.used_bytes() > first_charge);
    assert_cache_accounting(&cache);

    insert(&mut cache, b"same", empty_query());
    assert_eq!(cache.used_bytes(), first_charge);
    assert_cache_accounting(&cache);
}

#[test]
fn replacement_does_not_evict_a_different_recent_entry() {
    let mut probe = PreparedCache::new(1024);
    insert(&mut probe, b"a", empty_query());
    let one_charge = probe.used_bytes();
    let mut cache = PreparedCache::new(one_charge * 3);

    insert(&mut cache, b"a", empty_query());
    insert(&mut cache, b"b", empty_query());
    insert(&mut cache, b"a", empty_query());
    insert(&mut cache, b"c", empty_query());

    assert!(cache.get(b"a").is_some());
    assert!(cache.get(b"b").is_some());
    assert!(cache.get(b"c").is_some());
    assert_cache_accounting(&cache);
}

#[test]
fn simultaneous_misses_do_not_double_charge_one_key() {
    let cache = Arc::new(Mutex::new(PreparedCache::new(1024)));
    let both_missed = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let both_missed = Arc::clone(&both_missed);
            std::thread::spawn(move || {
                assert!(cache.lock().unwrap().get(b"same").is_none());
                both_missed.wait();
                insert(&mut cache.lock().unwrap(), b"same", empty_query());
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }

    let cache = cache.lock().unwrap();
    assert_eq!(cache.entries.len(), 1);
    assert_cache_accounting(&cache);
}

#[test]
fn retained_charge_includes_fts_literals_and_prepared_grams() {
    use crate::query::{CaseSensitivity, FtsMode, FtsQuery};

    let bind = |literal: &str| {
        crate::reference::bind_fts(
            &FtsQuery {
                case_sensitivity: CaseSensitivity::Sensitive,
                mode: FtsMode::Literal,
                literal: literal.into(),
                pattern_parameter: None,
                fields: Vec::new(),
            },
            &crate::reference::Schema::default(),
        )
        .unwrap()
    };
    let short = PreparedQuery {
        collections: vec![(0, None, Some(bind("ab")))],
    };
    let long = PreparedQuery {
        collections: vec![(0, None, Some(bind(&"abcd".repeat(256))))],
    };
    assert!(
        PreparedCache::entry_bytes(b"fts", &long)
            > PreparedCache::entry_bytes(b"fts", &short) + 1024
    );
}

#[test]
fn compiled_regex_charge_cannot_hide_inside_small_cache_budget() {
    use crate::query::{CaseSensitivity, FtsMode, FtsQuery, RegexOptions};

    let regex = crate::reference::bind_fts(
        &FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Regex(RegexOptions::default()),
            literal: "a+".into(),
            pattern_parameter: None,
            fields: Vec::new(),
        },
        &crate::reference::Schema::default(),
    )
    .unwrap();
    let query = PreparedQuery {
        collections: vec![(0, None, Some(regex))],
    };
    let charge = PreparedCache::entry_bytes(b"regex", &query);
    assert!(charge > 4 * 1024 * 1024);
    assert!(
        charge < 11 * 1024 * 1024,
        "the shared regex automaton was charged more than once"
    );
    let mut cache = PreparedCache::new(4 * 1024 * 1024);
    insert(&mut cache, b"regex", query);
    assert!(cache.get(b"regex").is_none());
    assert_eq!(cache.used_bytes(), 0);
}

#[test]
fn repeated_regex_preparation_reflects_the_chosen_cache_budget() {
    use crate::query::{CaseSensitivity, FtsMode, FtsQuery, QueryOptions, RegexOptions};
    use crate::repository::{CollectionDefinition, RepositoryOptions, SnapshotMode};
    use crate::source::SourceFormat;
    use crate::workload::ReturnMode;

    let scratch =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/prepared-cache-tests");
    std::fs::create_dir_all(&scratch).unwrap();
    let fixture = scratch.join(format!(
        "regex-reuse-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&fixture);
    std::fs::create_dir_all(fixture.join("data")).unwrap();
    std::fs::write(fixture.join("data/one.json"), br#"{"title":"abc"}"#).unwrap();
    let query = Query {
        collections: vec!["data".into()],
        filter: None,
        fts: Some(FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Regex(RegexOptions::default()),
            literal: "a+".into(),
            pattern_parameter: None,
            fields: Vec::new(),
        }),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 1,
            offset: 0,
            count: false,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    for (budget, expected_hits, expected_compilations) in [
        (4 * 1024 * 1024, [false, false], 2),
        (16 * 1024 * 1024, [false, true], 1),
    ] {
        let repo = crate::repository::Repository::open_with_options(
            &fixture,
            vec![CollectionDefinition::folder(
                "data",
                "data",
                SourceFormat::Json,
            )],
            RepositoryOptions {
                prepared_cache_bytes: budget,
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
        )
        .unwrap();
        let start = crate::regex_query::test_compile_count();
        let hits = [
            prepare(&repo, &query).unwrap().1,
            prepare(&repo, &query).unwrap().1,
        ];
        assert_eq!(hits, expected_hits);
        assert_eq!(
            crate::regex_query::test_compile_count() - start,
            expected_compilations
        );
    }
    std::fs::remove_dir_all(fixture).unwrap();
}
