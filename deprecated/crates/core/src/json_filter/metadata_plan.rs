//! Conservative metadata decisions over the file inventory.

use super::*;


/// A JSON or unavailable field cannot reject a source candidate.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetadataDecision {
    True,
    False,
    Unknown,
}


/// A necessary ID range for an AND branch. OR branches need a full scan.
pub(crate) enum MetadataIdHint {
    Exact(String),
    Filename(String),
    Prefix(String),
}


pub(crate) enum MetadataPathHint {
    Exact(String),
    Prefix(String),
}


impl JsonFilter {
    pub(crate) fn metadata_only(&self) -> bool {
        fn available(operand: &Operand) -> bool {
            match operand {
                Operand::Field(field) => {
                    field.modifier.is_none()
                        && matches!(
                            field.metadata,
                            Some(
                                MetadataField::Id
                                    | MetadataField::Filename
                                    | MetadataField::Path
                                    | MetadataField::Size
                                    | MetadataField::Modified
                            )
                        )
                }
                _ => true,
            }
        }

        fn all(expr: &Expr) -> bool {
            match expr {
                Expr::And(parts) | Expr::Or(parts) => parts.iter().all(all),
                Expr::Compare { left, right, .. } => available(left) && available(right),
                Expr::Exists(_) | Expr::Missing(_) | Expr::Relation(_) => false,
            }
        }
        self.has_metadata() && all(&self.expr)
    }

    pub(crate) fn metadata_decision(&self, metadata: &FileMetadata<'_>) -> MetadataDecision {
        fn available(operand: &Operand) -> bool {
            match operand {
                Operand::Field(field) => {
                    field.modifier.is_none()
                        && matches!(
                            field.metadata,
                            Some(
                                MetadataField::Id
                                    | MetadataField::Filename
                                    | MetadataField::Path
                                    | MetadataField::Size
                                    | MetadataField::Modified
                            )
                        )
                }
                _ => true,
            }
        }

        fn decide(expr: &Expr, metadata: &FileMetadata<'_>) -> MetadataDecision {
            match expr {
                Expr::And(parts) => {
                    let mut unknown = false;
                    for part in parts {
                        match decide(part, metadata) {
                            MetadataDecision::False => return MetadataDecision::False,
                            MetadataDecision::Unknown => unknown = true,
                            MetadataDecision::True => {}
                        }
                    }
                    if unknown {
                        MetadataDecision::Unknown
                    } else {
                        MetadataDecision::True
                    }
                }
                Expr::Or(parts) => {
                    let mut unknown = false;
                    for part in parts {
                        match decide(part, metadata) {
                            MetadataDecision::True => return MetadataDecision::True,
                            MetadataDecision::Unknown => unknown = true,
                            MetadataDecision::False => {}
                        }
                    }
                    if unknown {
                        MetadataDecision::Unknown
                    } else {
                        MetadataDecision::False
                    }
                }
                Expr::Compare {
                    left,
                    op,
                    any,
                    right,
                    ignore_case,
                } if available(left) && available(right) => {
                    let doc = Value::Null;
                    let a = left.select(&doc, Some(metadata), None);
                    let b = right.select(&doc, Some(metadata), None);
                    if a.is_empty() || b.is_empty() {
                        return MetadataDecision::False;
                    }
                    let mut pairs = a.iter().flat_map(|a| b.iter().map(move |b| (a, b)));
                    let test = |(a, b): (&SelectedValue<'_>, &SelectedValue<'_>)| {
                        compare_selected(
                            a,
                            *op,
                            b,
                            matches!(right, Operand::Field(_)),
                            *ignore_case,
                        )
                    };
                    let matches = if !any && (left.each() || right.each()) {
                        pairs.all(test)
                    } else {
                        pairs.any(test)
                    };
                    if matches {
                        MetadataDecision::True
                    } else {
                        MetadataDecision::False
                    }
                }
                _ => MetadataDecision::Unknown,
            }
        }
        decide(&self.expr, metadata)
    }

    pub(crate) fn metadata_id_hint(&self) -> Option<MetadataIdHint> {
        fn hint(expr: &Expr) -> Option<MetadataIdHint> {
            match expr {
                Expr::And(parts) => parts.iter().find_map(hint),
                Expr::Compare {
                    left,
                    op,
                    right,
                    ignore_case: false,
                    ..
                } => {
                    let (field, literal, field_left) = match (left, right) {
                        (Operand::Field(field), Operand::Literal(Value::String(value))) => {
                            (field, value, true)
                        }
                        (Operand::Literal(Value::String(value)), Operand::Field(field)) => {
                            (field, value, false)
                        }
                        _ => return None,
                    };
                    if field.modifier.is_some() {
                        return None;
                    }
                    if matches!(field.metadata, Some(MetadataField::Filename))
                        && matches!(op, Op::Eq)
                    {
                        return Some(MetadataIdHint::Filename(
                            literal.strip_suffix(".json")?.to_owned(),
                        ));
                    }
                    if !matches!(field.metadata, Some(MetadataField::Id)) {
                        return None;
                    }
                    if matches!(op, Op::Eq) {
                        return Some(MetadataIdHint::Exact(literal.to_owned()));
                    }
                    if field_left && matches!(op, Op::Contains) {
                        let prefix = pattern::Like::anchored_prefix(literal)?;
                        if !prefix.is_empty() {
                            return Some(MetadataIdHint::Prefix(prefix));
                        }
                    }
                    None
                }
                _ => None,
            }
        }
        hint(&self.expr)
    }

    pub(crate) fn metadata_path_hint(&self) -> Option<MetadataPathHint> {
        fn hint(expr: &Expr) -> Option<MetadataPathHint> {
            match expr {
                Expr::And(parts) => parts.iter().find_map(hint),
                Expr::Compare {
                    left: Operand::Field(field),
                    op,
                    right: Operand::Literal(Value::String(value)),
                    ignore_case: false,
                    ..
                } if field.metadata == Some(MetadataField::Path) && field.modifier.is_none() => {
                    match op {
                        Op::Eq => Some(MetadataPathHint::Exact(value.clone())),
                        Op::Contains => {
                            pattern::Like::anchored_prefix(value).map(MetadataPathHint::Prefix)
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        hint(&self.expr)
    }

    pub(crate) fn metadata_path_covers_root(&self, root: &str) -> bool {
        match &self.expr {
            Expr::Compare {
                left: Operand::Field(field),
                op: Op::Contains,
                right: Operand::Literal(Value::String(value)),
                ignore_case: false,
                ..
            } if field.metadata == Some(MetadataField::Path) && field.modifier.is_none() => {
                pattern::Like::anchored_prefix(value)
                    .is_some_and(|prefix| root.starts_with(&prefix))
            }
            _ => false,
        }
    }
}
