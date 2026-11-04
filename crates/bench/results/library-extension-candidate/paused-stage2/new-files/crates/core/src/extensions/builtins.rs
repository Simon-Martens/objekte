//! Public adapters for the renderers used by the concrete text matcher.

use super::*;
use crate::TextFormat;


/// Borrow the decoded string exactly as stored. This is the only public
/// representation currently eligible for the shared raw bigram index.
pub struct StoredRepresentation(Descriptor);


impl Default for StoredRepresentation {
    fn default() -> Self {
        Self(Descriptor::new(
            "objekte:stored",
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


impl TextRepresentation for StoredRepresentation {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }

    fn binds(&self, _: &BindingContext<'_>) -> bool {
        true
    }

    fn render<'a>(&self, value: &'a str, _: &'a mut String) -> Result<Option<&'a str>> {
        Ok(Some(value))
    }
}


/// Current visible-text renderers, made available through the public
/// representation contract. Rendering still uses the existing parsers.
pub struct RenderedRepresentation {
    descriptor: Descriptor,
    format: TextFormat,
}


impl RenderedRepresentation {
    pub fn new(format: TextFormat) -> Result<Self> {
        let name = match format {
            TextFormat::Plain => "plain",
            TextFormat::Markdown => "markdown",
            TextFormat::Html => "html",
            TextFormat::Xml => "xml",
        };
        Ok(Self {
            descriptor: Descriptor::new(
                format!("objekte:visible-{name}"),
                SemanticVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                &name,
                DataKind::Text,
                DataKind::Text,
                vec![
                    Dependency::SourceBytes,
                    Dependency::Schema("x-full-text".into()),
                ],
            ),
            format,
        })
    }
}


impl TextRepresentation for RenderedRepresentation {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn binds(&self, context: &BindingContext<'_>) -> bool {
        let expected = match self.format {
            TextFormat::Plain => "plain",
            TextFormat::Markdown => "markdown",
            TextFormat::Html => "html",
            TextFormat::Xml => "xml",
        };
        context.format == expected
    }

    fn render<'a>(&self, value: &'a str, output: &'a mut String) -> Result<Option<&'a str>> {
        if self.format == TextFormat::Plain && !crate::text::needs_normalizing(value) {
            return Ok(Some(value));
        }
        output.clear();
        Ok(crate::text::render_text_into(value, self.format, output).then_some(output.as_str()))
    }
}

/// The current word/raw/regex value scanner exposed through registration.
/// Pair `Word` with a visible representation and raw modes with `stored`.
pub struct NativeTextOperator {
    descriptor: Descriptor,
    mode: crate::TextMode,
    case_sensitive: bool,
}

impl NativeTextOperator {
    pub fn new(mode: crate::TextMode, case_sensitive: bool) -> Self {
        let name = match mode {
            crate::TextMode::Word => "word",
            crate::TextMode::WordRaw => "word-raw",
            crate::TextMode::Regex => "regex",
        };
        Self {
            descriptor: Descriptor::new(
                format!("objekte:{name}"),
                SemanticVersion { major: 1, minor: 0, patch: 0 },
                &(mode, case_sensitive),
                DataKind::Text,
                DataKind::Predicate,
                vec![Dependency::SourceBytes],
            ),
            mode,
            case_sensitive,
        }
    }
}

struct NativeCompiled {
    pattern: crate::text::TextPattern,
}

impl CompiledOperator for NativeCompiled {
    fn matches(&self, value: ScalarValue<'_>, _: &mut String) -> Result<bool> {
        Ok(matches!(value, ScalarValue::Text(text) if self.pattern.matches(text, TextFormat::Plain)))
    }

    fn candidate(&self) -> CandidateConstraint {
        if self.pattern.mode() == crate::TextMode::WordRaw {
            CandidateConstraint::BigramLiteral {
                literal: self.pattern.source().into(),
                representation: "objekte:stored".into(),
            }
        } else {
            CandidateConstraint::Unsupported
        }
    }
}

impl QueryOperator for NativeTextOperator {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn compile(&self, parameters: &Value) -> Result<Box<dyn CompiledOperator>> {
        let pattern = parameters.as_str().ok_or_else(|| {
            Error::Request("text operator pattern must be a string".into())
        })?;
        Ok(Box::new(NativeCompiled {
            pattern: crate::text::TextPattern::compile(pattern, self.mode, self.case_sensitive)?,
        }))
    }
}
