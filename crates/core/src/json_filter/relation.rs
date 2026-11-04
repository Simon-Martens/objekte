//! Paths that follow schema-declared relations into linked records.
//!
//! A step past an `x-relation` field behaves as if the linked record were
//! embedded there. Without a schema declaration, a path is ordinary JSON
//! traversal. A declared relation follows ID strings only; any other value
//! there is schema-invalid and reaches nothing, which keeps semi-joins exact.

use super::*;
use crate::{Field, TextFormat};
use std::{collections::HashMap, sync::Arc};


/// Top-level members needed per linked collection; `None` keeps whole records.
pub(crate) type LinkedKeys = BTreeMap<String, Option<BTreeSet<String>>>;


/// A path split where it crosses relations. `segments[k]` is read in the
/// record reached after `k` hops, and `targets[k]` names the collection that
/// the end of `segments[k]` links to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationPath {
    segments: Vec<JsonPath>,
    targets: Vec<String>,
    /// The declared rendering format of the final field in its collection.
    rendered: Option<TextFormat>,
}


impl RelationPath {
    /// Resolve `path` through declared relations of `collection`.
    pub(crate) fn resolve<'f>(
        collection: &str,
        path: &[JsonPathStep],
        fields_of: &dyn Fn(&str) -> Option<&'f BTreeMap<String, Field>>,
    ) -> Option<Self> {
        let mut segments = Vec::new();
        let mut targets = Vec::new();
        let mut start = 0;
        let mut root = fields_of(collection);
        let mut current: Option<&Field> = None;
        let mut index = 0;
        while index < path.len() {
            let more = index + 1 < path.len();
            let field = match &path[index] {
                JsonPathStep::Key(name) => {
                    if let Some(field) = current {
                        let mut field = field;
                        // JSON paths traverse arrays implicitly before a key.
                        while field.kind == crate::ValueType::Array {
                            match field.items.as_deref() {
                                Some(items) => field = items,
                                None => break,
                            }
                        }
                        field.properties.get(name)
                    } else {
                        root.and_then(|fields| fields.get(name))
                    }
                }
                JsonPathStep::Index(_) | JsonPathStep::Each => {
                    current.and_then(|field| field.items.as_deref())
                }
            };
            let Some(field) = field else {
                break;
            };
            // A relation on the field itself, or on the items of an array
            // that the next key traverses implicitly.
            let relation = field.relation.as_ref().or_else(|| {
                (field.kind == crate::ValueType::Array
                    && matches!(path.get(index + 1), Some(JsonPathStep::Key(_))))
                .then(|| {
                    field
                        .items
                        .as_deref()
                        .and_then(|items| items.relation.as_ref())
                })
                .flatten()
            });
            match relation {
                Some(target) if more => {
                    segments.push(path[start..=index].to_vec());
                    targets.push(target.clone());
                    start = index + 1;
                    root = fields_of(target);
                    current = None;
                }
                _ => current = Some(field),
            }
            index += 1;
        }
        if targets.is_empty() {
            return None;
        }
        let last = path[start..].to_vec();
        let rendered = fields_of(targets.last().expect("one hop"))
            .and_then(|fields| fts::declared_format(fields, &last));
        segments.push(last);
        Some(Self {
            segments,
            targets,
            rendered,
        })
    }

    /// The same path with its first hop removed, read in the first target.
    fn rest(&self) -> (JsonPath, Option<Self>) {
        let path = self.segments[1..].concat();
        let rest = (self.targets.len() > 1).then(|| Self {
            segments: self.segments[1..].to_vec(),
            targets: self.targets[1..].to_vec(),
            rendered: self.rendered,
        });
        (path, rest)
    }

    /// Top-level members each target collection needs; `None` keeps whole records.
    fn needed_keys(&self, keys: &mut LinkedKeys) {
        for (target, segment) in self.targets.iter().zip(&self.segments[1..]) {
            let entry = keys
                .entry(target.clone())
                .or_insert_with(|| Some(BTreeSet::new()));
            match (entry.as_mut(), segment.first()) {
                (Some(set), Some(JsonPathStep::Key(key))) => {
                    set.insert(key.clone());
                }
                _ => *entry = None,
            }
        }
    }
}


/// Resolved relation paths and the linked records one query reads.
#[derive(Debug, Default)]
pub(crate) struct RelationContext {
    paths: BTreeMap<(String, JsonPath), RelationPath>,
    tables: HashMap<String, HashMap<String, Value>>,
}


impl RelationContext {
    /// Resolve `paths` from `collection`, including the remaining paths in
    /// each linked collection, which semi-joins evaluate there.
    pub(crate) fn plan<'f>(
        collection: &str,
        paths: &[JsonPath],
        fields_of: &dyn Fn(&str) -> Option<&'f BTreeMap<String, Field>>,
    ) -> Option<(Self, LinkedKeys)> {
        let mut context = Self::default();
        let mut keys = BTreeMap::new();
        for path in paths {
            let Some(mut relation) = RelationPath::resolve(collection, path, fields_of) else {
                continue;
            };
            relation.needed_keys(&mut keys);
            let mut owner = collection.to_owned();
            let mut full = path.clone();
            loop {
                let (rest_path, rest) = relation.rest();
                let next = relation.targets[0].clone();
                context.paths.insert((owner, full), relation);
                let Some(rest) = rest else { break };
                owner = next;
                full = rest_path;
                relation = rest;
            }
        }
        (!context.paths.is_empty()).then_some((context, keys))
    }

    pub(crate) fn insert_table(&mut self, collection: String, records: HashMap<String, Value>) {
        self.tables.insert(collection, records);
    }

    pub(crate) fn path(&self, collection: &str, path: &JsonPath) -> Option<&RelationPath> {
        self.paths.get(&(collection.to_owned(), path.clone()))
    }

    /// The linked declared format of a relation path, if it is one.
    pub(crate) fn rendered(&self, collection: &str, path: &JsonPath) -> Option<Option<TextFormat>> {
        self.path(collection, path)
            .map(|relation| relation.rendered)
    }

    /// Values reached from `doc` in `collection`, or `None` for an ordinary path.
    pub(crate) fn values<'a>(
        &'a self,
        collection: &str,
        doc: &'a Value,
        path: &JsonPath,
        flatten: bool,
    ) -> Option<Vec<&'a Value>> {
        let relation = self.path(collection, path)?;
        let mut out = Vec::new();
        self.walk(doc, relation, 0, flatten, &mut out);
        Some(out)
    }

    fn walk<'a>(
        &'a self,
        doc: &'a Value,
        relation: &RelationPath,
        hop: usize,
        flatten: bool,
        out: &mut Vec<&'a Value>,
    ) {
        let segment = &relation.segments[hop];
        if hop == relation.targets.len() {
            out.extend(values(doc, segment, flatten));
            return;
        }
        let table = self.tables.get(&relation.targets[hop]);
        for value in values(doc, segment, true) {
            if let Value::String(id) = value
                && let Some(linked) = table.and_then(|records| records.get(id))
            {
                self.walk(linked, relation, hop + 1, flatten, out);
            }
        }
    }

    /// Linked IDs whose record satisfies `leaf` existentially at the rest of
    /// its relation path. A scalar comparison returns `None` when a linked
    /// record yields several values, because then only the full scan can
    /// raise the cardinality error.
    fn matching_targets(
        &self,
        collection: &str,
        leaf: &Expr,
    ) -> Option<(JsonPath, BTreeSet<String>)> {
        let (path, sub, scalar) = match leaf {
            Expr::Exists(path) => {
                let relation = self.path(collection, path)?;
                (path, Expr::Exists(relation.rest().0), false)
            }
            Expr::Compare {
                left,
                op,
                any,
                right,
                ignore_case,
            } => {
                let (field, literal_left) = match (left, right) {
                    (Operand::Field(field), other) if !matches!(other, Operand::Field(_)) => {
                        (field, false)
                    }
                    (other, Operand::Field(field)) if !matches!(other, Operand::Field(_)) => {
                        (field, true)
                    }
                    _ => return None,
                };
                let relation = self.path(collection, &field.path)?;
                let operand = Operand::Field(FieldOperand {
                    path: relation.rest().0,
                    // `:each` needs a nonempty selection, so one matching
                    // value in a linked record is still necessary.
                    modifier: field
                        .modifier
                        .filter(|modifier| *modifier != Modifier::Each),
                    metadata: None,
                });
                let (left, right) = if literal_left {
                    (left.clone(), operand)
                } else {
                    (operand, right.clone())
                };
                (
                    &field.path,
                    Expr::Compare {
                        left,
                        op: *op,
                        any: true,
                        right,
                        ignore_case: *ignore_case,
                    },
                    !*any && field.modifier != Some(Modifier::Each),
                )
            }
            _ => return None,
        };
        let relation = self.path(collection, path)?;
        let target = &relation.targets[0];
        let records = self.tables.get(target)?;
        let (rest, _) = relation.rest();
        let mut ids = BTreeSet::new();
        for (id, record) in records {
            let reached = self
                .values(target, record, &rest, true)
                .unwrap_or_else(|| values(record, &rest, true));
            if scalar && reached.len() > 1 {
                return None;
            }
            if evaluate(&sub, record, None, Some((target, self))) {
                ids.insert(id.clone());
            }
        }
        Some((relation.segments[0].clone(), ids))
    }
}


impl JsonFilter {
    /// Mark comparisons that follow relations from `collection`; they are
    /// evaluated through `context` and skipped by index planners.
    pub(crate) fn bind_relations(&self, collection: &str, context: Arc<RelationContext>) -> Self {
        fn rendered(field: &mut FieldOperand, relation: &RelationPath) {
            if field.modifier == Some(Modifier::Fts) {
                field.modifier = Some(
                    relation
                        .rendered
                        .map_or(Modifier::Normalized, Modifier::Rendered),
                );
            }
        }
        fn bind(expr: &mut Expr, collection: &str, context: &RelationContext) {
            let crosses = |path: &JsonPath| context.path(collection, path).is_some();
            match expr {
                Expr::And(parts) | Expr::Or(parts) => {
                    for part in parts {
                        bind(part, collection, context);
                    }
                }
                Expr::Exists(path) | Expr::Missing(path) if crosses(path) => {
                    *expr = Expr::Relation(Box::new(expr.clone()));
                }
                Expr::Compare { left, right, .. } => {
                    let mut related = false;
                    for operand in [left, right] {
                        if let Operand::Field(field) = operand
                            && field.metadata.is_none()
                            && let Some(relation) = context.path(collection, &field.path)
                        {
                            rendered(field, relation);
                            related = true;
                        }
                    }
                    if related {
                        *expr = Expr::Relation(Box::new(expr.clone()));
                    }
                }
                Expr::Exists(_) | Expr::Missing(_) | Expr::Relation(_) => {}
            }
        }
        let mut bound = self.clone();
        bind(&mut bound.expr, collection, &context);
        for field in &mut bound.scalar_paths {
            if let Some(relation) = context.path(collection, &field.path) {
                rendered(field, relation);
            }
        }
        bound.relations = Some((collection.to_owned(), context));
        bound
    }

    pub(crate) fn has_relations(&self) -> bool {
        self.relations.is_some()
    }

    pub(crate) fn is_relation_path(&self, path: &JsonPath) -> bool {
        self.relations
            .as_ref()
            .is_some_and(|(collection, context)| context.path(collection, path).is_some())
    }

    /// Necessary semi-joins: each is a source path whose values must include
    /// one of the IDs. `scalar_safe` reports whether a source path yields at
    /// most one value, so that skipping records cannot hide a cardinality error.
    pub(crate) fn relation_semijoins(
        &self,
        scalar_safe: impl Fn(&JsonPath) -> bool,
    ) -> Vec<(JsonPath, BTreeSet<String>)> {
        let Some((collection, context)) = &self.relations else {
            return Vec::new();
        };
        let terms: Vec<&Expr> = match &self.expr {
            Expr::And(parts) => parts.iter().collect(),
            expr => vec![expr],
        };
        terms
            .into_iter()
            .filter_map(|term| match term {
                Expr::Relation(leaf) => {
                    let scalar = matches!(
                        leaf.as_ref(),
                        Expr::Compare { any: false, left, right, .. }
                            if !left.each() && !right.each()
                    );
                    let (path, ids) = context.matching_targets(collection, leaf)?;
                    (!scalar || scalar_safe(&path)).then_some((path, ids))
                }
                _ => None,
            })
            .collect()
    }
}
