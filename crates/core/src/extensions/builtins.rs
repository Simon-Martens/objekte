//! Public adapters for the renderers used by the concrete text matcher.

use super::*;
use crate::TextFormat;


/// The default JSON format exposed through the public decoder contract.
/// Production JSON indexing continues to use its fused Sonic projection.
pub struct JsonFormatDecoder(Descriptor);


impl Default for JsonFormatDecoder {
    fn default() -> Self {
        Self(Descriptor::new(
            "objekte:json",
            SemanticVersion {
                major: 1,
                minor: 0,
                patch: 0,
            },
            &(),
            DataKind::Source,
            DataKind::Record,
            vec![Dependency::SourceBytes],
        ))
    }
}


impl FormatDecoder for JsonFormatDecoder {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }

    fn accepts(&self, source: &SourceDescriptor) -> bool {
        source.format == self.0.identity
    }

    fn decode(
        &self,
        bytes: &[u8],
        visit: &mut dyn FnMut(&dyn DecodedRecord) -> Result<()>,
    ) -> Result<()> {
        match self.decode_outcome(bytes, visit)? {
            DecodeOutcome::Decoded => Ok(()),
            DecodeOutcome::Malformed(diagnostics) => Err(Error::MalformedSource {
                format: self.0.identity.clone(),
                message: diagnostics[0].message.clone(),
            }),
        }
    }

    fn decode_outcome(
        &self,
        bytes: &[u8],
        visit: &mut dyn FnMut(&dyn DecodedRecord) -> Result<()>,
    ) -> Result<DecodeOutcome> {
        match sonic_rs::from_slice::<SonicValue>(bytes) {
            Ok(value) => {
                visit(&value)?;
                Ok(DecodeOutcome::Decoded)
            }
            Err(error) => Ok(DecodeOutcome::Malformed(vec![FormatDiagnostic::new(
                "json_syntax",
                error.to_string(),
            )])),
        }
    }

    fn malformed_text<'a>(&self, bytes: &'a [u8]) -> Result<Option<std::borrow::Cow<'a, str>>> {
        Ok(Some(std::borrow::Cow::Owned(
            crate::text::malformed_search_text(bytes),
        )))
    }
}


impl DecodedRecord for SonicValue {
    fn visit_leaves(
        &self,
        visit: &mut dyn FnMut(&str, ScalarValue<'_>) -> Result<()>,
    ) -> Result<()> {
        fn walk(
            value: &SonicValue,
            path: &mut String,
            depth: usize,
            visit: &mut dyn FnMut(&str, ScalarValue<'_>) -> Result<()>,
        ) -> Result<()> {
            if depth > 127 {
                return Err(Error::Request("JSON source exceeds nesting limit".into()));
            }
            if let Some(number) = value.as_raw_number() {
                return visit(path, ScalarValue::Number(number.as_str()));
            }
            match value.as_ref() {
                ValueRef::Null => visit(path, ScalarValue::Null),
                ValueRef::Bool(value) => visit(path, ScalarValue::Bool(value)),
                ValueRef::String(value) => visit(path, ScalarValue::Text(value)),
                ValueRef::Number(_) => unreachable!("raw number handled first"),
                ValueRef::Array(values) => {
                    for (index, value) in values.iter().enumerate() {
                        let prior = path.len();
                        path.push('/');
                        path.push_str(&index.to_string());
                        walk(value, path, depth + 1, visit)?;
                        path.truncate(prior);
                    }
                    Ok(())
                }
                ValueRef::Object(values) => {
                    // Sonic retains duplicate members. A logical JSON record
                    // uses the last occurrence, as the native verifier does.
                    let mut members = Vec::new();
                    let mut last = BTreeMap::new();
                    for (key, value) in values {
                        last.insert(key, members.len());
                        members.push((key, value));
                    }
                    for (index, (key, value)) in members.into_iter().enumerate() {
                        if last[key] != index {
                            continue;
                        }
                        let prior = path.len();
                        path.push('/');
                        for character in key.chars() {
                            match character {
                                '~' => path.push_str("~0"),
                                '/' => path.push_str("~1"),
                                _ => path.push(character),
                            }
                        }
                        walk(value, path, depth + 1, visit)?;
                        path.truncate(prior);
                    }
                    Ok(())
                }
            }
        }
        walk(self, &mut String::new(), 0, visit)
    }

    fn visit_nodes(
        &self,
        visit: &mut dyn FnMut(&str, LogicalNode<'_>) -> Result<()>,
    ) -> Result<()> {
        fn walk(
            value: &SonicValue,
            path: &mut String,
            depth: usize,
            visit: &mut dyn FnMut(&str, LogicalNode<'_>) -> Result<()>,
        ) -> Result<()> {
            if depth > 127 {
                return Err(Error::Request("JSON source exceeds nesting limit".into()));
            }
            if let Some(number) = value.as_raw_number() {
                return visit(
                    path,
                    LogicalNode::Scalar(ScalarValue::Number(number.as_str())),
                );
            }
            match value.as_ref() {
                ValueRef::Null => visit(path, LogicalNode::Scalar(ScalarValue::Null)),
                ValueRef::Bool(value) => visit(path, LogicalNode::Scalar(ScalarValue::Bool(value))),
                ValueRef::String(value) => {
                    visit(path, LogicalNode::Scalar(ScalarValue::Text(value)))
                }
                ValueRef::Number(_) => unreachable!("raw number handled first"),
                ValueRef::Array(values) => {
                    visit(path, LogicalNode::Array)?;
                    for (index, value) in values.iter().enumerate() {
                        let prior = path.len();
                        path.push('/');
                        path.push_str(&index.to_string());
                        walk(value, path, depth + 1, visit)?;
                        path.truncate(prior);
                    }
                    Ok(())
                }
                ValueRef::Object(values) => {
                    visit(path, LogicalNode::Object)?;
                    let mut members = Vec::new();
                    let mut last = BTreeMap::new();
                    for (key, value) in values {
                        last.insert(key, members.len());
                        members.push((key, value));
                    }
                    for (index, (key, value)) in members.into_iter().enumerate() {
                        if last[key] != index {
                            continue;
                        }
                        let prior = path.len();
                        path.push('/');
                        for character in key.chars() {
                            match character {
                                '~' => path.push_str("~0"),
                                '/' => path.push_str("~1"),
                                _ => path.push(character),
                            }
                        }
                        walk(value, path, depth + 1, visit)?;
                        path.truncate(prior);
                    }
                    Ok(())
                }
            }
        }
        walk(self, &mut String::new(), 0, visit)
    }
}


impl DecodedRecord for Value {
    fn visit_leaves(
        &self,
        visit: &mut dyn FnMut(&str, ScalarValue<'_>) -> Result<()>,
    ) -> Result<()> {
        fn walk(
            value: &Value,
            path: &mut String,
            depth: usize,
            visit: &mut dyn FnMut(&str, ScalarValue<'_>) -> Result<()>,
        ) -> Result<()> {
            if depth > 127 {
                return Err(Error::Request("JSON source exceeds nesting limit".into()));
            }
            match value {
                Value::Null => visit(path, ScalarValue::Null),
                Value::Bool(value) => visit(path, ScalarValue::Bool(*value)),
                Value::Number(value) => visit(path, ScalarValue::Number(value.as_str())),
                Value::String(value) => visit(path, ScalarValue::Text(value)),
                Value::Array(values) => {
                    for (index, value) in values.iter().enumerate() {
                        let prior = path.len();
                        path.push('/');
                        path.push_str(&index.to_string());
                        walk(value, path, depth + 1, visit)?;
                        path.truncate(prior);
                    }
                    Ok(())
                }
                Value::Object(values) => {
                    for (key, value) in values {
                        let prior = path.len();
                        path.push('/');
                        for character in key.chars() {
                            match character {
                                '~' => path.push_str("~0"),
                                '/' => path.push_str("~1"),
                                _ => path.push(character),
                            }
                        }
                        walk(value, path, depth + 1, visit)?;
                        path.truncate(prior);
                    }
                    Ok(())
                }
            }
        }
        walk(self, &mut String::new(), 0, visit)
    }

    fn visit_nodes(
        &self,
        visit: &mut dyn FnMut(&str, LogicalNode<'_>) -> Result<()>,
    ) -> Result<()> {
        fn walk(
            value: &Value,
            path: &mut String,
            depth: usize,
            visit: &mut dyn FnMut(&str, LogicalNode<'_>) -> Result<()>,
        ) -> Result<()> {
            if depth > 127 {
                return Err(Error::Request("JSON source exceeds nesting limit".into()));
            }
            match value {
                Value::Null => visit(path, LogicalNode::Scalar(ScalarValue::Null)),
                Value::Bool(value) => visit(path, LogicalNode::Scalar(ScalarValue::Bool(*value))),
                Value::Number(value) => visit(
                    path,
                    LogicalNode::Scalar(ScalarValue::Number(value.as_str())),
                ),
                Value::String(value) => visit(path, LogicalNode::Scalar(ScalarValue::Text(value))),
                Value::Array(values) => {
                    visit(path, LogicalNode::Array)?;
                    for (index, value) in values.iter().enumerate() {
                        let prior = path.len();
                        path.push('/');
                        path.push_str(&index.to_string());
                        walk(value, path, depth + 1, visit)?;
                        path.truncate(prior);
                    }
                    Ok(())
                }
                Value::Object(values) => {
                    visit(path, LogicalNode::Object)?;
                    for (key, value) in values {
                        let prior = path.len();
                        path.push('/');
                        for character in key.chars() {
                            match character {
                                '~' => path.push_str("~0"),
                                '/' => path.push_str("~1"),
                                _ => path.push(character),
                            }
                        }
                        walk(value, path, depth + 1, visit)?;
                        path.truncate(prior);
                    }
                    Ok(())
                }
            }
        }
        walk(self, &mut String::new(), 0, visit)
    }
}


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

    fn native_index_projection(&self) -> Option<NativeTextProjection> {
        Some(NativeTextProjection::Stored)
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

    fn preparation(&self) -> TextPreparation {
        TextPreparation::VisibleNormalized
    }

    fn native_index_projection(&self) -> Option<NativeTextProjection> {
        Some(NativeTextProjection::VisibleNormalized(self.format))
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
                SemanticVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
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
        Ok(
            matches!(value, ScalarValue::Text(text) if self.pattern.matches(text, TextFormat::Plain)),
        )
    }

    fn matches_prepared(&self, value: ScalarValue<'_>, _: &mut String) -> Result<bool> {
        Ok(matches!(value, ScalarValue::Text(text) if self.pattern.matches_prepared(text)))
    }

    fn candidate(&self) -> CandidateConstraint {
        match self.pattern.mode() {
            crate::TextMode::WordRaw => CandidateConstraint::BigramLiteral {
                literal: self.pattern.needle().expect("word mode has needle").into(),
                representation: "objekte:stored".into(),
            },
            crate::TextMode::Word => CandidateConstraint::BigramLiteral {
                literal: self.pattern.needle().expect("word mode has needle").into(),
                representation: "objekte:visible-normalized".into(),
            },
            crate::TextMode::Regex => CandidateConstraint::Unsupported,
        }
    }

    fn can_skip_non_candidates(&self) -> bool {
        true
    }
}


impl QueryOperator for NativeTextOperator {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn compile(&self, parameters: &Value) -> Result<Box<dyn CompiledOperator>> {
        let pattern = parameters
            .as_str()
            .ok_or_else(|| Error::Request("text operator pattern must be a string".into()))?;
        Ok(Box::new(NativeCompiled {
            pattern: crate::text::TextPattern::compile(pattern, self.mode, self.case_sensitive)?,
        }))
    }
}
