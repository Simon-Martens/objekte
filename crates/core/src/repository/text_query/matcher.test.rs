use super::*;
use crate::{
    TextFormat,
    extensions::{
        BindingContext, DataKind, Dependency, Descriptor, Extensions, NativeTextOperator,
        RenderedRepresentation, SemanticVersion, StoredRepresentation, TextRepresentation,
    },
    text::take_normalization_calls,
};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};


struct CountingRepresentation {
    descriptor: Descriptor,
    binds: Arc<AtomicUsize>,
}


impl TextRepresentation for CountingRepresentation {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn binds(&self, _: &BindingContext<'_>) -> bool {
        self.binds.fetch_add(1, Ordering::Relaxed);
        true
    }

    fn render<'a>(&self, value: &'a str, _: &'a mut String) -> crate::Result<Option<&'a str>> {
        Ok(Some(value))
    }
}


#[test]
fn declared_and_unanticipated_paths_bind_once_per_plan() {
    let schema =
        json!({"type":"object","properties":{"items":{"type":"array","items":{"type":"string"}}}});
    let compiled =
        crate::schema::Schema::compile(&schema, std::path::Path::new("schema.json")).unwrap();
    let rules = TextRules::new(&compiled.field.properties);
    let binds = Arc::new(AtomicUsize::new(0));
    let mut definitions = Extensions::default();
    definitions
        .register_representation(CountingRepresentation {
            descriptor: Descriptor::new(
                "test:counted",
                SemanticVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                &(),
                DataKind::Text,
                DataKind::Text,
                vec![Dependency::SourceBytes],
            ),
            binds: Arc::clone(&binds),
        })
        .unwrap();
    definitions
        .register_operator(NativeTextOperator::new(TextMode::WordRaw, true))
        .unwrap();
    let extension = definitions
        .bind_text("test:counted", "objekte:word-raw", &json!("alpha"))
        .unwrap();
    let plan = TextBindingPlan::new("books", Some(&schema), &rules, &extension);
    assert_eq!(binds.load(Ordering::Relaxed), 2);
    let document = json!({"items":["alpha","alpha"],"extra":{"text":"alpha"}});
    for _ in 0..2 {
        let found = TextMatcher::matching_fields_extension(
            &document, &rules, None, true, &extension, &plan,
        )
        .unwrap();
        assert_eq!(found.fields, ["extra.text", "items[]"]);
    }
    assert_eq!(binds.load(Ordering::Relaxed), 3);
}

#[test]
fn prepared_text_is_normalized_once_and_stored_word_keeps_legacy_behavior() {
    let mut definitions = Extensions::default();
    definitions
        .register_representation(RenderedRepresentation::new(TextFormat::Plain).unwrap())
        .unwrap();
    definitions
        .register_representation(StoredRepresentation::default())
        .unwrap();
    definitions
        .register_operator(NativeTextOperator::new(TextMode::Word, false))
        .unwrap();
    let visible = definitions
        .bind_text(
            "objekte:visible-plain",
            "objekte:word",
            &json!("alpha beta"),
        )
        .unwrap();
    let stored = definitions
        .bind_text("objekte:stored", "objekte:word", &json!("alpha beta"))
        .unwrap();
    let borrowed = definitions
        .bind_text("objekte:visible-plain", "objekte:word", &json!("alpha"))
        .unwrap();
    let rules = TextRules::new(&BTreeMap::new());
    let visible_binding = TextBindingPlan::new("books", None, &rules, &visible);
    let stored_binding = TextBindingPlan::new("books", None, &rules, &stored);
    let borrowed_binding = TextBindingPlan::new("books", None, &rules, &borrowed);
    let punctuated = json!({"body": "Alpha,Beta"});

    take_normalization_calls();
    let result = TextMatcher::matching_fields_extension(
        &punctuated,
        &rules,
        None,
        false,
        &visible,
        &visible_binding,
    )
    .unwrap();
    assert_eq!(result.fields, ["body"]);
    assert_eq!(take_normalization_calls(), 1);

    let result = TextMatcher::matching_fields_extension(
        &punctuated,
        &rules,
        None,
        false,
        &stored,
        &stored_binding,
    )
    .unwrap();
    assert_eq!(result.fields, ["body"]);
    assert_eq!(take_normalization_calls(), 1);

    let plain = json!({"body": "Alpha"});
    let result = TextMatcher::matching_fields_extension(
        &plain,
        &rules,
        None,
        false,
        &borrowed,
        &borrowed_binding,
    )
    .unwrap();
    assert_eq!(result.fields, ["body"]);
    assert_eq!(take_normalization_calls(), 0);
}
