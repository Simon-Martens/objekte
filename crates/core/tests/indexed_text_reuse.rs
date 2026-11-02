//! Indexed scalar values are verified through their B-tree postings in text searches.

use objekte_core::{
    CacheMode, JsonQuery, JsonQueryBackend, Options, Repository, TextMode, TextQuery,
    bigram::BigramSnapshot,
};
use serde_json::json;
use std::{collections::BTreeSet, fs, path::Path};

fn fixture(rendered: bool) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".objekte/collections")).unwrap();
    fs::create_dir_all(root.join("data/books")).unwrap();
    let mut schema = json!({
        "type":"object",
        "properties":{
            "category":{"type":"string","index":true},
            "rank":{"type":"integer","index":true},
            "flag":{"type":"boolean","index":true},
            "note":{"type":"string"},
            "markup":{"type":"string"}
        },
        "x-objekte":{"name":"books","path":"data/books","type":"data","format":"json","id":"filename"}
    });
    if rendered {
        schema["properties"]["markup"]["x-full-text"] = json!("html");
        schema["properties"]["category"]["x-full-text"] = json!("html");
    }
    fs::write(
        root.join(".objekte/collections/books.schema.json"),
        schema.to_string(),
    )
    .unwrap();
    for (id, category, rank, flag, note, markup) in [
        (
            "a",
            "ZXQmarker Müller",
            234567,
            true,
            "ordinary",
            "<b>visible</b>",
        ),
        ("b", "Other", 7, false, "othernote", "<i>plain</i>"),
        ("c", "Äpfel", 8, true, "ordinary", "<b>third</b>"),
        ("d", "last", 9, false, "ordinary", "<b>fourth</b>"),
        ("e", "<b>He</b>lp", 10, false, "ordinary", "plain"),
    ] {
        fs::write(
            root.join(format!("data/books/{id}.json")),
            json!({"category":category,"rank":rank,"flag":flag,"note":note,"markup":markup})
                .to_string(),
        )
        .unwrap();
    }
    fs::write(
        root.join("data/books/f.json"),
        r#"{"category":["arrayneedle"],"note":"ordinary"}"#,
    )
    .unwrap();
    temp
}

fn repository(root: &Path, cache: CacheMode) -> Repository {
    let mut options = Options::new();
    options.cache = cache;
    Repository::load_repository(root, options).unwrap()
}

fn text_ids(
    repo: &mut Repository,
    pattern: &str,
    mode: TextMode,
    fields: Option<&str>,
) -> (BTreeSet<String>, usize) {
    let result = repo
        .search_text(&TextQuery {
            pattern: pattern.into(),
            mode,
            fields: fields.map(|field| vec![field.into()]),
            collections: Some(vec!["books".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap();
    (
        result.hits.into_iter().map(|hit| hit.record.id).collect(),
        result.total.unwrap(),
    )
}

fn filter_ids(
    repo: &mut Repository,
    filter: &str,
    backend: JsonQueryBackend,
) -> (BTreeSet<String>, usize) {
    let result = repo
        .query_json(&JsonQuery {
            filter: filter.into(),
            backend,
            collections: Some(vec!["books".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap();
    (
        result.records.into_iter().map(|record| record.id).collect(),
        result.total.unwrap(),
    )
}

#[test]
fn btree_scalars_reach_text_and_filter_verification() {
    for rendered in [false, true] {
        let temp = fixture(rendered);
        let mut indexed = repository(temp.path(), CacheMode::Memory);
        let mut direct = repository(temp.path(), CacheMode::Disabled);
        for (pattern, mode, fields) in [
            ("ZXQmarker", TextMode::Word, Some("category")),
            ("zxqMARKER", TextMode::Word, Some("category")),
            ("234567", TextMode::Word, Some("rank")),
            ("true", TextMode::Word, Some("flag")),
            ("ZXQmarker", TextMode::Word, None),
            ("Müller", TextMode::Word, Some("category")),
            ("^ZXQ.*Müller$", TextMode::RegexClean, Some("category")),
            ("visible", TextMode::Word, Some("markup")),
            ("Help", TextMode::Word, Some("category")),
            ("arrayneedle", TextMode::Word, None),
        ] {
            assert_eq!(
                text_ids(&mut indexed, pattern, mode, fields),
                text_ids(&mut direct, pattern, mode, fields),
                "rendered={rendered} pattern={pattern} fields={fields:?}"
            );
        }
        assert_eq!(
            text_ids(&mut indexed, "Help", TextMode::Word, Some("category")).1,
            usize::from(rendered)
        );
        for filter in [
            "category ~ 'ZXQmarker'",
            "category ~ '%marker%'",
            "category ~ :ignorecase('%MÜLLER%')",
            "category:fts ~ 'ZXQmarker'",
            "rank = 234567",
            "flag = true",
            "category ~ '%marker%' || note ~ 'othernote'",
            "category ~ '%marker%' && flag = true",
            "category:fts ~ 'Help'",
        ] {
            assert_eq!(
                filter_ids(&mut indexed, filter, JsonQueryBackend::Bigram),
                filter_ids(&mut direct, filter, JsonQueryBackend::Direct),
                "rendered={rendered} filter={filter}"
            );
        }
        assert_eq!(
            filter_ids(
                &mut indexed,
                "category:fts ~ 'Help'",
                JsonQueryBackend::Bigram
            )
            .1,
            usize::from(rendered)
        );
        let cache = indexed.save_bigram_cache("books").unwrap();
        assert_eq!(&fs::read(&cache).unwrap()[..8], b"OBJSNP04");
        let snapshot = BigramSnapshot::load(&cache).unwrap();
        assert!(!snapshot.filter.has_key(u16::from_be_bytes(*b"zx")));
        let mut restored = repository(temp.path(), CacheMode::Memory);
        assert!(restored.restore_cached_index_unchecked("books").unwrap());
        assert_eq!(
            text_ids(&mut restored, "ZXQmarker", TextMode::Word, Some("category")),
            text_ids(&mut direct, "ZXQmarker", TextMode::Word, Some("category")),
        );
        assert_eq!(
            filter_ids(
                &mut restored,
                "category ~ '%marker%'",
                JsonQueryBackend::Bigram
            ),
            filter_ids(
                &mut direct,
                "category ~ '%marker%'",
                JsonQueryBackend::Direct
            ),
        );
        fs::write(
            temp.path().join("data/books/a.json"),
            r#"{"category":"brandnewmarker","rank":234567,"flag":true,"note":"ordinary","markup":"plain"}"#,
        )
        .unwrap();
        restored.refresh("books").unwrap();
        assert_eq!(
            text_ids(
                &mut restored,
                "brandnewmarker",
                TextMode::Word,
                Some("category")
            )
            .1,
            1
        );
        assert_eq!(
            text_ids(&mut restored, "ZXQmarker", TextMode::Word, Some("category")).1,
            0
        );
        assert_eq!(
            filter_ids(
                &mut restored,
                "category ~ '%brandnew%'",
                JsonQueryBackend::Bigram
            )
            .1,
            1
        );
        let cache = restored.save_bigram_cache("books").unwrap();
        let mut resumed = repository(temp.path(), CacheMode::Memory);
        assert!(resumed.restore_cached_index_unchecked("books").unwrap());
        assert_eq!(
            text_ids(
                &mut resumed,
                "brandnewmarker",
                TextMode::Word,
                Some("category")
            )
            .1,
            1
        );
        assert_eq!(&fs::read(cache).unwrap()[..8], b"OBJSNP04");
        fs::write(
            temp.path().join("data/books/g.json"),
            r#"{"category":"freshnewvalue","rank":11,"flag":true,"note":"ordinary"}"#,
        )
        .unwrap();
        fs::remove_file(temp.path().join("data/books/a.json")).unwrap();
        resumed.refresh("books").unwrap();
        assert_eq!(
            text_ids(
                &mut resumed,
                "freshnewvalue",
                TextMode::Word,
                Some("category")
            )
            .1,
            1
        );
        assert_eq!(
            text_ids(
                &mut resumed,
                "brandnewmarker",
                TextMode::Word,
                Some("category")
            )
            .1,
            0
        );
    }
}

#[test]
fn legacy_projection_cache_is_rebuilt_before_search() {
    let temp = fixture(false);
    let mut first = repository(temp.path(), CacheMode::Memory);
    let cache = first.save_bigram_cache("books").unwrap();
    let mut bytes = fs::read(&cache).unwrap();
    bytes[..8].copy_from_slice(b"OBJSNP03");
    fs::write(&cache, bytes).unwrap();

    let mut reopened = repository(temp.path(), CacheMode::Memory);
    assert!(!reopened.restore_cached_index_unchecked("books").unwrap());
    assert_eq!(
        text_ids(&mut reopened, "ZXQmarker", TextMode::Word, Some("category")).1,
        1
    );
    assert_eq!(&fs::read(&cache).unwrap()[..8], b"OBJSNP04");
}

#[test]
fn changing_indexed_fields_rebuilds_the_base_projection() {
    let temp = fixture(false);
    let schema_path = temp.path().join(".objekte/collections/books.schema.json");
    let mut first = repository(temp.path(), CacheMode::Memory);
    let cache = first.save_bigram_cache("books").unwrap();
    assert!(
        !BigramSnapshot::load(&cache)
            .unwrap()
            .filter
            .has_key(u16::from_be_bytes(*b"zx"))
    );

    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    schema["properties"]["category"]["index"] = json!(false);
    fs::write(&schema_path, schema.to_string()).unwrap();
    let mut unindexed = repository(temp.path(), CacheMode::Memory);
    assert_eq!(
        text_ids(
            &mut unindexed,
            "ZXQmarker",
            TextMode::Word,
            Some("category")
        )
        .1,
        1
    );
    assert!(
        BigramSnapshot::load(&cache)
            .unwrap()
            .filter
            .has_key(u16::from_be_bytes(*b"zx"))
    );

    schema["properties"]["category"]["index"] = json!(true);
    fs::write(&schema_path, schema.to_string()).unwrap();
    let mut reindexed = repository(temp.path(), CacheMode::Memory);
    assert_eq!(
        text_ids(
            &mut reindexed,
            "ZXQmarker",
            TextMode::Word,
            Some("category")
        )
        .1,
        1
    );
    assert!(
        !BigramSnapshot::load(&cache)
            .unwrap()
            .filter
            .has_key(u16::from_be_bytes(*b"zx"))
    );
}
