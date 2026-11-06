//! Explicit query path steps, separate from indexable schema field names.

use crate::model::{FieldPath, PathRoot};


/// One data-field or array occurrence step in a query path.
#[derive(Clone, Debug, Eq, PartialEq, Hash, serde::Serialize)]
pub enum PathStep {
    Field(String),
    ArrayIndex(u32),
    EachElement,
}


/// Parsed path with its schema lookup key and occurrence-preserving steps.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct QueryPath {
    pub field: FieldPath,
    pub steps: Vec<PathStep>,
    pub has_explicit_array_step: bool,
    pub has_position: bool,
    pub display: String,
}


impl QueryPath {
    pub fn parse(input: &str) -> Result<Self, String> {
        if let Some(rest) = input.strip_prefix("@backlinks.") {
            let mut path = Self::parse(rest)?;
            if path.field.root != PathRoot::Data {
                return Err("backlink path requires an alias".into());
            }
            path.field.root = PathRoot::Backlinks;
            path.display = input.into();
            return Ok(path);
        }
        if input.starts_with('@') {
            let field = FieldPath::parse(input)?;
            return Ok(Self {
                display: field.display(),
                field,
                steps: Vec::new(),
                has_explicit_array_step: false,
                has_position: false,
            });
        }
        let mut steps = Vec::new();
        let mut segments = Vec::new();
        let mut rest = input;
        let mut has_explicit_array_step = false;
        let mut has_position = false;
        let mut after_step = false;
        while !rest.is_empty() {
            if rest.starts_with('.') {
                if !after_step {
                    return Err("empty path segment".into());
                }
                rest = &rest[1..];
                if rest.is_empty() {
                    return Err("trailing path separator".into());
                }
                after_step = false;
                continue;
            }
            if rest.starts_with("[\"") || rest.starts_with("['") {
                let quote = rest.as_bytes()[1] as char;
                let mut escaped = false;
                let mut closing_quote = None;
                for (offset, ch) in rest[2..].char_indices() {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == quote {
                        closing_quote = Some(offset + 2);
                        break;
                    }
                }
                let end = closing_quote.ok_or("unterminated quoted path")? + 2;
                let fragment = rest.get(..end).ok_or("invalid quoted path boundary")?;
                let parsed = FieldPath::parse(fragment)?;
                let name = parsed
                    .segments
                    .into_iter()
                    .next()
                    .ok_or("empty quoted path")?;
                segments.push(name.clone());
                steps.push(PathStep::Field(name));
                rest = &rest[end..];
                after_step = true;
                continue;
            }
            if rest.starts_with('[') {
                if !after_step {
                    return Err("array selector needs a preceding field".into());
                }
                let end = rest.find(']').ok_or("unclosed array selector")?;
                let selector = &rest[1..end];
                let step = if selector == "*" {
                    PathStep::EachElement
                } else if !selector.is_empty() && selector.bytes().all(|byte| byte.is_ascii_digit())
                {
                    has_position = true;
                    PathStep::ArrayIndex(
                        selector
                            .parse::<u32>()
                            .map_err(|_| "array index exceeds u32")?,
                    )
                } else {
                    return Err(format!("invalid array selector [{selector}]"));
                };
                steps.push(step);
                has_explicit_array_step = true;
                rest = &rest[end + 1..];
                after_step = true;
                continue;
            }
            if after_step {
                return Err("invalid path separator".into());
            }
            let end = rest.find(['.', '[']).unwrap_or(rest.len());
            if end == 0 {
                return Err("empty path segment".into());
            }
            let name = rest[..end].to_owned();
            segments.push(name.clone());
            steps.push(PathStep::Field(name));
            rest = &rest[end..];
            after_step = true;
        }
        if segments.is_empty() {
            return Err("empty field path".into());
        }
        let field = FieldPath {
            root: PathRoot::Data,
            segments,
        };
        Ok(Self {
            display: input.into(),
            field,
            steps,
            has_explicit_array_step,
            has_position,
        })
    }
}


#[cfg(test)]
#[path = "query_path.test.rs"]
mod tests;
