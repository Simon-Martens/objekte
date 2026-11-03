use super::*;
use objekte_core::Repository;
use serde_json::{Value, json};
use std::{fs, path::Path};


fn write(root: &Path, relative: &str, value: Value) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn schema(root: &Path, name: &str, properties: Value) {
    write(
        root,
        &format!(".objekte/collections/{name}.schema.json"),
        json!({
            "type":"object",
            "properties": properties,
            "x-objekte":{"name":name,"path":format!("data/{name}"),"type":"data","format":"json","id":"filename"}
        }),
    );
}

/// `books` links to `people`; `people` has a nested object and an array.
fn repository() -> (tempfile::TempDir, Repository) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    schema(
        root,
        "books",
        json!({
            "title":{"type":"string"},
            "year":{"type":"integer"},
            "status":{"type":"string","enum":["draft","published","out of print"]},
            "author":{"type":"string","x-relation":{"collection":"people"}},
            "tags":{"type":"array","items":{"type":"string"}},
            "printed":{"type":"boolean"},
            "first edition":{"type":"string"}
        }),
    );
    schema(
        root,
        "people",
        json!({
            "name":{"type":"string"},
            "born":{"type":"integer"},
            "address":{"type":"object","properties":{"city":{"type":"string"},"zip":{"type":"string"}}}
        }),
    );
    write(root, "data/books/a.json", json!({"title":"A"}));
    write(root, "data/people/p.json", json!({"name":"P"}));
    let repo = Repository::open(root).unwrap();
    (temp, repo)
}

fn labels(text: &str, scope: Option<&str>) -> Vec<String> {
    let (_temp, repo) = repository();
    let schema = Schema::new(&repo.model().collections, scope);
    complete(text, text.chars().count(), &schema)
        .map(|found| found.items.into_iter().map(|item| item.label).collect())
        .unwrap_or_default()
}

/// Apply the first suggestion, as Tab or Enter would.
fn accept(text: &str, scope: Option<&str>) -> String {
    let (_temp, repo) = repository();
    let schema = Schema::new(&repo.model().collections, scope);
    let found = complete(text, text.chars().count(), &schema).expect("a suggestion");
    let mut input = super::super::input::Input::with_text(text);
    input.replace(found.start, found.end, &found.items[0].insert);
    input.text
}


#[test]
fn empty_filter_lists_fields_system_fields_and_functions() {
    let found = labels("", Some("books"));
    for expected in ["title", "year", "author", "@id", "@modified", "exists("] {
        assert!(
            found.iter().any(|label| label == expected),
            "{expected} in {found:?}"
        );
    }
}

#[test]
fn prefix_matches_rank_before_substring_matches() {
    let found = labels("t", Some("books"));
    let title = found.iter().position(|label| label == "title").unwrap();
    let status = found.iter().position(|label| label == "status").unwrap();
    assert!(title < status, "{found:?}");
}

#[test]
fn relations_complete_into_the_linked_collection() {
    assert_eq!(accept("author.na", Some("books")), "author.name");
    let found = labels("author.", Some("books"));
    assert!(found.contains(&"born".to_string()), "{found:?}");
    assert!(found.contains(&"address".to_string()), "{found:?}");
    assert_eq!(
        accept("author.address.c", Some("books")),
        "author.address.city"
    );
}

#[test]
fn a_complete_field_name_steps_into_its_children() {
    let found = labels("author", Some("books"));
    assert!(found.contains(&"name".to_string()), "{found:?}");
    assert_eq!(accept("author", Some("books")), "author.address");
}

#[test]
fn keys_that_are_not_identifiers_use_brackets() {
    assert_eq!(accept("first", Some("books")), "[\"first edition\"]");
}

#[test]
fn operators_follow_the_field_type() {
    let numbers = labels("year ", Some("books"));
    assert!(numbers.contains(&">=".to_string()));
    assert!(!numbers.contains(&"~".to_string()), "{numbers:?}");
    let flags = labels("printed ", Some("books"));
    assert_eq!(flags, ["=", "!="]);
    let tags = labels("tags ", Some("books"));
    assert!(tags.contains(&"?=".to_string()), "{tags:?}");
    let text = labels("title ", Some("books"));
    assert!(text.contains(&"~".to_string()), "{text:?}");
}

#[test]
fn enum_values_are_suggested_quoted_and_inside_quotes() {
    let found = labels("status = ", Some("books"));
    assert!(found.contains(&"published".to_string()), "{found:?}");
    assert_eq!(
        accept("status = pub", Some("books")),
        "status = 'published'"
    );
    assert_eq!(
        accept("status = 'out", Some("books")),
        "status = 'out of print'"
    );
}

#[test]
fn date_macros_only_follow_date_like_fields() {
    assert!(labels("@modified > ", None).contains(&"@now".to_string()));
    assert!(!labels("year > ", Some("books")).contains(&"@now".to_string()));
    assert!(labels("title > ", Some("books")).contains(&"@now".to_string()));
}

#[test]
fn values_are_followed_by_connectives_and_sort() {
    let found = labels("year > 1800 ", Some("books"));
    assert_eq!(found, ["&&", "||", "SORT BY"]);
    let sort = labels("year > 1800 SORT BY ", Some("books"));
    assert!(sort.contains(&"year".to_string()), "{sort:?}");
    let direction = labels("year > 1800 SORT BY year ", Some("books"));
    assert!(direction.contains(&"DESC".to_string()), "{direction:?}");
}

#[test]
fn modifiers_complete_after_a_colon() {
    let found = labels("title:", Some("books"));
    assert!(found.contains(&"fts".to_string()), "{found:?}");
    assert!(found.contains(&"lower".to_string()), "{found:?}");
}

#[test]
fn scope_limits_fields_to_one_collection() {
    let books = labels("", Some("books"));
    assert!(!books.contains(&"born".to_string()));
    let all = labels("", None);
    assert!(all.contains(&"born".to_string()));
    assert!(all.contains(&"year".to_string()));
}
