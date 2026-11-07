use super::*;
use crate::executor::fts_candidates;
use crate::model::{FieldPath, LogicalType, RecordSlot};
use crate::query::{FtsField, FtsMode, FtsQuery, Query, QueryOptions};
use crate::reference::{
    BindingOperation, CustomBinding, FieldDefinition, RegisteredBinding, Schema, Transform,
    bind_fts,
};
use crate::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use crate::source::{SourceFormat, TextSegment};
use crate::workload::{BigramAlphabet, ReturnMode};

#[test]
fn compilation_limits_and_flags() {
    let plain = CompiledRegex::compile(
        "^foo.bar$",
        RegexOptions::default(),
        CaseSensitivity::Sensitive,
    )
    .unwrap();
    assert!(plain.is_match("foo-bar"));
    assert!(!plain.is_match("foo\nbar"));
    assert!(!plain.is_match("FOO-BAR"));

    let insensitive = CompiledRegex::compile(
        "^foo.bar$",
        RegexOptions {
            multi_line: true,
            dot_matches_new_line: true,
        },
        CaseSensitivity::Insensitive,
    )
    .unwrap();
    assert!(insensitive.is_match("FOO\nBAR"));
    assert!(insensitive.is_match("prefix\nFOO-BAR\nsuffix"));

    assert!(
        CompiledRegex::compile("(", RegexOptions::default(), CaseSensitivity::Sensitive)
            .unwrap_err()
            .contains("invalid regex")
    );
    assert!(
        CompiledRegex::compile("a\\1", RegexOptions::default(), CaseSensitivity::Sensitive)
            .unwrap_err()
            .contains("invalid regex")
    );
    assert!(
        CompiledRegex::compile(
            &"a".repeat(MAX_PATTERN_BYTES + 1),
            RegexOptions::default(),
            CaseSensitivity::Sensitive
        )
        .unwrap_err()
        .contains("byte limit")
    );
}

#[test]
fn regex_fts_is_segment_scoped_mode_aware_and_cache_safe() {
    let root = std::env::temp_dir().join(format!("objekte-regex-fts-{}", std::process::id()));
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let lines = [
        serde_json::json!({"title":"foo", "body":"bar"}).to_string(),
        serde_json::json!({"title":"foo-bar", "body":"x"}).to_string(),
        serde_json::json!({"title":"FOO\nBAR", "body":"x"}).to_string(),
        serde_json::json!({"title":"Straße", "body":"x"}).to_string(),
        serde_json::json!({"title":"MÜLLER", "body":"x"}).to_string(),
        "{\"title\":needle".into(),
    ];
    std::fs::write(data.join("records.jsonl"), lines.join("\n")).unwrap();
    let mut schema = Schema::default();
    for name in ["title", "body"] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed: false,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    schema
        .register_custom(
            vec!["title".into()],
            CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: Vec::new(),
        sort_locale: "en".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let repo = Repository::open_with_options(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                bigram_alphabet: alphabet,
                snapshot_mode: SnapshotMode::IfValid,
                verified_cache_bytes: 1024 * 1024,
                ..Default::default()
            },
        )
        .unwrap();
        let query = |pattern: &str, selected: Option<&str>, case_sensitivity, options| Query {
            collections: vec!["data".into()],
            fts: Some(FtsQuery {
                pattern_parameter: None,
                case_sensitivity,
                mode: FtsMode::Regex(options),
                literal: pattern.into(),
                fields: selected
                    .into_iter()
                    .map(|name| FtsField {
                        query_path: None,
                        path: FieldPath::parse(name).unwrap(),
                        binding: None,
                    })
                    .collect(),
            }),
            filter: None,
            options: QueryOptions {
                sort: Vec::new(),
                limit: 20,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Logical,
                evidence: true,
            },
        };
        for (request, expected) in [
            (
                query(
                    "^foo.bar$",
                    Some("title"),
                    CaseSensitivity::Sensitive,
                    RegexOptions::default(),
                ),
                1,
            ),
            (
                query(
                    "^foo.bar$",
                    Some("title"),
                    CaseSensitivity::Insensitive,
                    RegexOptions {
                        multi_line: true,
                        dot_matches_new_line: true,
                    },
                ),
                2,
            ),
            (
                query(
                    "foo.*bar",
                    None,
                    CaseSensitivity::Sensitive,
                    RegexOptions::default(),
                ),
                1,
            ),
            (
                query(
                    "^bar$",
                    Some("body"),
                    CaseSensitivity::Sensitive,
                    RegexOptions::default(),
                ),
                1,
            ),
            (
                query(
                    "\\{\\\"title\\\":needle",
                    None,
                    CaseSensitivity::Sensitive,
                    RegexOptions::default(),
                ),
                1,
            ),
        ] {
            let optimized = repo.search(&request).unwrap();
            let reference = repo.search_reference(&request).unwrap();
            assert_eq!(optimized.count, Some(expected), "{alphabet:?} {request:?}");
            assert_eq!(optimized.hits, reference.hits, "{alphabet:?} {request:?}");
            assert_eq!(repo.search(&request).unwrap().hits, optimized.hits);
        }
        let selective = query(
            "foo.*bar",
            None,
            CaseSensitivity::Sensitive,
            RegexOptions::default(),
        );
        let plan = repo.explain(&selective).unwrap();
        // Folded grams admit a mixed-case distractor; exact regex rejects it.
        assert_eq!(plan[0].fts_estimated_candidates, Some(3));
        assert_eq!(
            plan[0].fts_mode,
            Some(FtsMode::Regex(RegexOptions::default()))
        );
        assert!(
            plan[0]
                .fts_prefilter_reason
                .as_ref()
                .unwrap()
                .contains("necessary folded grams")
        );
        let bound = bind_fts(selective.fts.as_ref().unwrap(), &definition.schema).unwrap();
        assert_eq!(
            fts_candidates(&repo.collections()[0], &bound)
                .unwrap()
                .len(),
            3
        );
        // The record with separate "foo" and "bar" segments is a candidate,
        // but cannot satisfy the exact regex within either segment.
        assert_eq!(repo.search(&selective).unwrap().count, Some(1));
        let unrestricted = query(
            "foo|.*",
            None,
            CaseSensitivity::Sensitive,
            RegexOptions::default(),
        );
        let plan = repo.explain(&unrestricted).unwrap();
        assert_eq!(plan[0].fts_estimated_candidates, None);
        assert!(
            plan[0]
                .fts_prefilter_reason
                .as_ref()
                .unwrap()
                .contains("no bounded necessary literal")
        );
        let sensitive = query(
            "^foo",
            Some("title"),
            CaseSensitivity::Sensitive,
            RegexOptions::default(),
        );
        let insensitive = query(
            "^foo",
            Some("title"),
            CaseSensitivity::Insensitive,
            RegexOptions::default(),
        );
        assert_eq!(repo.search(&sensitive).unwrap().count, Some(2));
        let mut folded = query(
            "^strasse$",
            Some("title"),
            CaseSensitivity::Sensitive,
            RegexOptions::default(),
        );
        folded.fts.as_mut().unwrap().fields[0].binding = Some("folded".into());
        assert_eq!(repo.search(&folded).unwrap().count, Some(1));
        assert_eq!(
            repo.search(&folded).unwrap().hits,
            repo.search_reference(&folded).unwrap().hits
        );
        let bound = bind_fts(folded.fts.as_ref().unwrap(), &definition.schema).unwrap();
        assert!(fts_candidates(&repo.collections()[0], &bound).is_some());
        let fallback = query(
            "\\{\\\"title\\\":needle",
            None,
            CaseSensitivity::Sensitive,
            RegexOptions::default(),
        );
        let bound = bind_fts(fallback.fts.as_ref().unwrap(), &definition.schema).unwrap();
        let fallback_slot = repo.collections()[0]
            .generation
            .records
            .iter()
            .find(|record| record.class == crate::inventory::RecordClass::RawFallback)
            .unwrap()
            .slot
            .0;
        assert!(
            fts_candidates(&repo.collections()[0], &bound)
                .unwrap()
                .contains(fallback_slot)
        );
        assert_eq!(repo.search(&insensitive).unwrap().count, Some(3));
        let inline_case = query(
            "(?i:^foo)",
            Some("title"),
            CaseSensitivity::Sensitive,
            RegexOptions::default(),
        );
        assert_eq!(repo.search(&inline_case).unwrap().count, Some(3));
        let unicode_case = query(
            "(?i:^müller$)",
            Some("title"),
            CaseSensitivity::Sensitive,
            RegexOptions::default(),
        );
        assert_eq!(repo.search(&unicode_case).unwrap().count, Some(1));
        assert_eq!(
            repo.search(&unicode_case).unwrap().hits,
            repo.search_reference(&unicode_case).unwrap().hits
        );
        assert_eq!(repo.search(&sensitive).unwrap().count, Some(2));
        let invalid = query(
            "(",
            None,
            CaseSensitivity::Sensitive,
            RegexOptions::default(),
        );
        assert!(repo.search(&invalid).unwrap_err().contains("invalid regex"));
        repo.save_default_snapshot_for_test();
        drop(repo);
        let restored = Repository::open_with_options(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                bigram_alphabet: alphabet,
                snapshot_mode: SnapshotMode::IfValid,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(restored.startup().restored);
        let request = query(
            "^foo.bar$",
            Some("title"),
            CaseSensitivity::Sensitive,
            RegexOptions::default(),
        );
        assert_eq!(restored.search(&request).unwrap().count, Some(1));
        assert_eq!(
            restored.search(&request).unwrap().hits,
            restored.search_reference(&request).unwrap().hits
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn generated_regex_matches_remain_inside_conservative_gram_candidates() {
    let fixed: Vec<Vec<&str>> = vec![
        vec![""],
        vec!["foo"],
        vec!["bar"],
        vec!["foobar"],
        vec!["fooXbar"],
        vec!["FOObar"],
        vec!["Straße"],
        vec!["STRASSE"],
        vec!["ß"],
        vec!["ss"],
        vec!["aßb"],
        vec!["abc"],
        vec!["aübc"],
        vec!["a😀bc"],
        vec!["Σσς"],
        vec!["Iİıi"],
        vec!["a\nb"],
        vec!["foo", "bar"],
        vec!["ab", "cd"],
    ];
    let mut sources: Vec<Vec<String>> = fixed
        .into_iter()
        .map(|parts| parts.into_iter().map(str::to_owned).collect())
        .collect();
    const SEED: u64 = 0x5EED_2026_1106;
    let mut state = SEED;
    let chars = ['a', 'b', 'ß', 'ü', 'Σ', '😀'];
    for _ in 0..128 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let length = ((state >> 32) % 9) as usize;
        let mut text = String::new();
        for _ in 0..length {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            text.push(chars[((state >> 32) as usize) % chars.len()]);
        }
        sources.push(vec![text]);
    }
    let patterns = [
        "foo.*bar",
        "foo|bar",
        "(?:foo)?bar",
        "ß",
        "ss",
        "aßb",
        "^abc$",
        ".*",
        "a?",
        r"\bfoo\b",
        "I|İ|ı",
        "Σ|σ|ς",
        "a.b",
        "a\\nb",
        "(?i:foo)",
        "foo|.*",
        "^foo|bar$",
    ];
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut index = BigramIndex::new(alphabet);
        for (slot, parts) in sources.iter().enumerate() {
            let segments = parts
                .iter()
                .map(|part| TextSegment {
                    path: vec!["body".into()],
                    text: part.clone(),
                })
                .collect::<Vec<_>>();
            index
                .add_record(RecordSlot(slot as u32), &segments)
                .unwrap();
        }
        for pattern in patterns {
            for case_sensitivity in [CaseSensitivity::Sensitive, CaseSensitivity::Insensitive] {
                let compiled =
                    CompiledRegex::compile(pattern, RegexOptions::default(), case_sensitivity)
                        .unwrap();
                let Some(candidates) = compiled
                    .condition()
                    .and_then(|condition| condition.candidates(&index))
                else {
                    continue;
                };
                let independent = regex::RegexBuilder::new(pattern)
                    .case_insensitive(case_sensitivity == CaseSensitivity::Insensitive)
                    .build()
                    .unwrap();
                for (slot, parts) in sources.iter().enumerate() {
                    if parts.iter().any(|part| independent.is_match(part)) {
                        assert!(
                            candidates.contains(slot as u32),
                            "seed={SEED:x} {alphabet:?} {case_sensitivity:?} {pattern:?} {parts:?}"
                        );
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
struct RegexMapping {
    compatible: bool,
}

impl BindingOperation for RegexMapping {
    fn accepts(
        &self,
        _kind: &LogicalType,
        _operator: crate::query::Operator,
        _multi: bool,
    ) -> bool {
        true
    }

    fn prepare_literal(
        &self,
        _literal: &crate::query::Literal,
        _kind: &LogicalType,
    ) -> Result<crate::model::Value, String> {
        Err("not used by regex".into())
    }

    fn map_value(&self, _value: &crate::model::Value) -> Option<crate::model::Value> {
        None
    }

    fn map_text(&self, text: &str) -> String {
        text.replace('-', "")
    }

    fn regex_text_compatible(&self) -> bool {
        self.compatible
    }
}

#[test]
fn custom_regex_binding_requires_explicit_representation_contract() {
    let fts = FtsQuery {
        pattern_parameter: None,
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Regex(RegexOptions::default()),
        literal: "foobar".into(),
        fields: Vec::new(),
    };
    for compatible in [false, true] {
        let mut schema = Schema::default();
        schema
            .set_raw_binding(RegisteredBinding {
                name: "dashless".into(),
                version: 1,
                operation: Arc::new(RegexMapping { compatible }),
            })
            .unwrap();
        if compatible {
            let bound = bind_fts(&fts, &schema).unwrap();
            assert!(bound.raw_matcher.unwrap().matches("foo-bar").unwrap());
            assert!(bound.fallback_regex.as_ref().unwrap().is_match("foobar"));
            assert!(!bound.fallback_regex.as_ref().unwrap().is_match("foo-bar"));
        } else {
            assert!(
                bind_fts(&fts, &schema)
                    .unwrap_err()
                    .contains("does not support regex mode")
            );
        }
    }
}

#[test]
fn custom_raw_regex_uses_mapped_index_and_original_fallback_after_restore() {
    let root = std::env::temp_dir().join(format!("objekte-regex-raw-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/records.jsonl"),
        [
            serde_json::json!({"title": "foo-bar"}).to_string(),
            serde_json::json!({"title": "fooXbar"}).to_string(),
            "{\"title\":\"foobar".into(),
        ]
        .join("\n"),
    )
    .unwrap();
    let mut schema = Schema::default();
    schema
        .set_raw_binding(RegisteredBinding {
            name: "dashless".into(),
            version: 1,
            operation: Arc::new(RegexMapping { compatible: true }),
        })
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: Vec::new(),
        sort_locale: "en".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let query = |pattern: &str| Query {
        collections: vec!["data".into()],
        fts: Some(FtsQuery {
            pattern_parameter: None,
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Regex(RegexOptions::default()),
            literal: pattern.into(),
            fields: Vec::new(),
        }),
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    };
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::IfValid,
            verified_cache_bytes: 1024 * 1024,
            ..Default::default()
        };
        let built =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        assert!(!built.startup().restored);
        assert_eq!(built.build_failed_parses(), 1);
        let built_query = query("foobar");
        assert_eq!(built.search(&built_query).unwrap().count, Some(2));
        assert_eq!(
            built.search(&built_query).unwrap().hits,
            built.search_reference(&built_query).unwrap().hits
        );
        built.save_default_snapshot_for_test();
        drop(built);
        let restored =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        assert!(restored.startup().restored);
        let collection = &restored.collections()[0];
        let fallback_slot = collection
            .generation
            .records
            .iter()
            .find(|record| record.class == crate::inventory::RecordClass::RawFallback)
            .unwrap()
            .slot
            .0;
        for (pattern, expected) in [("^foobar$", 1), ("foobar", 2), ("foo-bar", 0)] {
            let request = query(pattern);
            let bound = bind_fts(request.fts.as_ref().unwrap(), &definition.schema).unwrap();
            let candidates = fts_candidates(collection, &bound).unwrap();
            if pattern == "foobar" {
                assert_eq!(candidates.len(), 2, "{alphabet:?}");
                assert!(candidates.contains(fallback_slot));
            }
            let optimized = restored.search(&request).unwrap();
            let reference = restored.search_reference(&request).unwrap();
            assert_eq!(optimized.count, Some(expected), "{alphabet:?} {pattern}");
            assert_eq!(optimized.hits, reference.hits, "{alphabet:?} {pattern}");
            if pattern == "foobar" {
                assert!(optimized.hits.iter().any(|hit| {
                    hit.record_class == crate::inventory::RecordClass::RawFallback
                        && hit.matched_fields == ["@record.raw_text"]
                }));
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
