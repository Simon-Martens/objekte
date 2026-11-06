//! A public-API-only, JSON-source example: register, warm, query, save,
//! reopen, restore, refresh, and query again.

use objekte_core::{
    JsonQuery, Options, Repository, Result, TextFieldSelector, TextQuery,
    extensions::{
        BindingContext, DataKind, Dependency, Descriptor, SemanticVersion, TextComparison,
        TextPreparation, TextRepresentation, TextSearchDefinition,
    },
};
use std::{fs, path::Path};


struct ExpandedAbbreviations(Descriptor);


impl ExpandedAbbreviations {
    fn new() -> Self {
        Self(Descriptor::new(
            "example:expanded-abbreviations-values",
            SemanticVersion {
                major: 1,
                minor: 0,
                patch: 0,
            },
            &(),
            DataKind::Text,
            DataKind::Text,
            vec![Dependency::SourceBytes],
        ))
    }
}


impl TextRepresentation for ExpandedAbbreviations {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }

    fn binds(&self, context: &BindingContext<'_>) -> bool {
        context.path == "body"
    }

    fn preparation(&self) -> TextPreparation {
        TextPreparation::Stored
    }

    fn render<'a>(&self, value: &'a str, output: &'a mut String) -> Result<Option<&'a str>> {
        if !value.contains("MS") {
            return Ok(Some(value));
        }
        output.clear();
        output.push_str(&value.replace("MS", "manuscript"));
        Ok(Some(output))
    }
}


fn open(root: &Path) -> Result<Repository> {
    let definition = TextSearchDefinition::new(
        Descriptor::new(
            "example:expanded-abbreviations",
            SemanticVersion {
                major: 1,
                minor: 0,
                patch: 0,
            },
            &(),
            DataKind::Text,
            DataKind::Predicate,
            vec![Dependency::SourceBytes],
        ),
        "example:expanded-abbreviations-values",
    )
    .alias("abbreviations")
    .operator(TextComparison::Contains, "objekte:word-raw")
    .operator(TextComparison::Equal, "objekte:word-raw");
    Repository::builder(root)
        .options(Options::new())
        .register_representation(ExpandedAbbreviations::new())?
        .register_text_definition(definition)?
        .open()
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path();
    fs::create_dir_all(root.join("data/docs"))?;
    fs::create_dir_all(root.join(".objekte/collections"))?;
    fs::write(
        root.join(".objekte/collections/docs.schema.json"),
        r#"{
      "type":"object",
      "properties": {
        "body": {"type":"string", "x-text-search": {
          "default":"example:expanded-abbreviations",
          "definitions": {
            "example:expanded-abbreviations":{"index":true},
            "objekte:markdown":{"index":true}
          }
        }},
        "year":{"type":"integer","index":true}
      },
      "x-objekte":{"name":"docs","path":"data/docs","type":"data",
                   "format":"json","id":"filename"}
    }"#,
    )?;
    fs::write(
        root.join("data/docs/a.json"),
        br#"{"body":"An MS witness","year":1848}"#,
    )?;
    fs::write(
        root.join("data/docs/b.json"),
        br#"{"body":"A later note","year":1850}"#,
    )?;

    let text = TextQuery {
        pattern: "manuscript".into(),
        fields: Some(vec![TextFieldSelector::with_search(
            "body",
            "abbreviations",
        )]),
        count: true,
        ..Default::default()
    };
    let filter = JsonQuery {
        filter: "body:abbreviations ~ 'manuscript' && year = 1848".into(),
        count: true,
        ..Default::default()
    };
    let mut repository = open(root)?;
    repository.warm_text_index("docs")?;
    assert_eq!(repository.search_text(&text)?.total, Some(1));
    assert_eq!(repository.query_json(&filter)?.total, Some(1));
    repository.save_bigram_cache("docs")?;
    drop(repository);

    let mut reopened = open(root)?;
    reopened.warm_text_index("docs")?;
    assert_eq!(reopened.query_json(&filter)?.total, Some(1));
    fs::write(
        root.join("data/docs/a.json"),
        br#"{"body":"A repaired witness","year":1848}"#,
    )?;
    reopened.refresh("docs")?;
    assert_eq!(reopened.search_text(&text)?.total, Some(0));
    println!("registered text definition: warm, query, restore, and refresh passed");
    Ok(())
}
