//! Query-facing text definitions compose a representation and exact operators.

use super::*;
use std::collections::BTreeMap;


/// The positive comparison implemented by a text operator. Filter negation
/// uses the same exact result, without complementing approximate candidates.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextComparison {
    Contains,
    Equal,
}


/// How the query expression is interpreted before exact matching.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSyntax {
    Literal,
    Regex,
}


/// Explicit search settings. `None` preserves the next lower-precedence
/// setting; `Some(false)` is an intentional override, not an omission.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextSearchOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub syntax: Option<TextSyntax>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case_sensitive: Option<bool>,
}


impl TextSearchOptions {
    pub fn is_default(&self) -> bool {
        self.syntax.is_none() && self.case_sensitive.is_none()
    }
}


/// Parameters sent to a registered operator at binding, once per term.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextCompileRequest {
    pub pattern: String,
    pub comparison: TextComparison,
    pub syntax: TextSyntax,
    /// `None` lets the definition's operator retain its declared default.
    pub case_sensitive: Option<bool>,
}


/// One query-facing interpretation of a logical string field. Operation
/// names identify registered operators; the engine owns all index storage.
#[derive(Clone, Debug)]
pub struct TextSearchDefinition {
    descriptor: Descriptor,
    representation: String,
    aliases: Vec<String>,
    pub(super) operators: BTreeMap<TextComparison, String>,
    pub(super) regex_operator: Option<String>,
}


impl TextSearchDefinition {
    pub fn new(descriptor: Descriptor, representation: impl Into<String>) -> Self {
        Self {
            descriptor,
            representation: representation.into(),
            aliases: Vec::new(),
            operators: BTreeMap::new(),
            regex_operator: None,
        }
    }

    pub fn alias(mut self, alias: impl Into<String>) -> Self {
        self.aliases.push(alias.into());
        self
    }

    pub fn operator(mut self, comparison: TextComparison, identity: impl Into<String>) -> Self {
        self.operators.insert(comparison, identity.into());
        self
    }

    pub fn regex_operator(mut self, identity: impl Into<String>) -> Self {
        self.regex_operator = Some(identity.into());
        self
    }

    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    pub fn representation(&self) -> &str {
        &self.representation
    }

    pub fn aliases(&self) -> &[String] {
        &self.aliases
    }

    pub fn operator_for(&self, request: &TextCompileRequest) -> Option<&str> {
        match request.syntax {
            TextSyntax::Literal => self.operators.get(&request.comparison).map(String::as_str),
            TextSyntax::Regex if request.comparison == TextComparison::Contains => {
                self.regex_operator.as_deref()
            }
            TextSyntax::Regex => None,
        }
    }
}
