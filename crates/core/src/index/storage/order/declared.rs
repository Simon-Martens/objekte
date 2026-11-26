//! Schema and collation declarations for an empty collection domain.

use std::borrow::Cow;
use std::collections::HashMap;

use icu_collator::CollatorBorrowed;

use super::{
    COLLATION_VERSION, CompositeSortPart, OrderColumnReader, SortConfig, SortKey, SortOptions,
    make_collator, simple_composite_part,
};
use crate::index::storage::ordered_slots::{EMPTY_ORDER, OrderedSlots};
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::model::{FieldPath, LogicalType, RecordSlot};
use crate::query::options::SortField;
use crate::schema::Schema;

pub(crate) struct DeclaredOrders {
    columns: HashMap<FieldPath, DeclaredOrder>,
    composites: Vec<Vec<CompositeSortPart>>,
}

impl DeclaredOrders {
    pub(crate) fn new(
        paths: &[FieldPath],
        schema: &Schema,
        locale: &str,
        defaults: SortOptions,
        overrides: &HashMap<FieldPath, SortConfig>,
    ) -> Result<Self, String> {
        validate_overrides(paths, overrides)?;
        let mut columns = HashMap::with_capacity(paths.len());
        for path in paths {
            let (config, kind, _) = order_configuration(path, schema, locale, defaults, overrides)?;
            let order = DeclaredOrder {
                path: path.clone(),
                kind,
                config,
            };
            if columns.insert(path.clone(), order).is_some() {
                return Err("duplicate sort field".into());
            }
        }
        let composites = composite_orders(schema, |path| columns.contains_key(path))?;
        Ok(Self {
            columns,
            composites,
        })
    }

    pub(crate) fn column(&self, path: &FieldPath) -> Option<&dyn OrderColumnReader> {
        self.columns
            .get(path)
            .map(|column| column as &dyn OrderColumnReader)
    }

    pub(crate) fn composite(&self, fields: &[SortField]) -> Option<&dyn OrderedSlots> {
        let requested = fields
            .iter()
            .map(simple_composite_part)
            .collect::<Option<Vec<_>>>()?;
        self.composites.contains(&requested).then_some(&EMPTY_ORDER)
    }
}

struct DeclaredOrder {
    path: FieldPath,
    kind: LogicalType,
    config: SortConfig,
}

impl OrderColumnReader for DeclaredOrder {
    fn path(&self) -> &FieldPath {
        &self.path
    }
    fn kind(&self) -> &LogicalType {
        &self.kind
    }
    fn collation_version(&self) -> &str {
        COLLATION_VERSION
    }
    fn options(&self) -> SortOptions {
        self.config.options
    }
    fn locale(&self) -> &str {
        &self.config.locale
    }
    fn key_count(&self) -> usize {
        0
    }
    fn retained_bytes(&self) -> usize {
        0
    }

    fn key(&self, _: RecordSlot) -> StorageResult<Cow<'_, SortKey>> {
        Err(StorageError::invalid(
            "read declared sort key",
            "empty order has no slots",
        ))
    }

    fn slots(&self, _: bool) -> StorageResult<&dyn OrderedSlots> {
        Ok(&EMPTY_ORDER)
    }
}

pub(crate) fn validate_overrides(
    paths: &[FieldPath],
    overrides: &HashMap<FieldPath, SortConfig>,
) -> Result<(), String> {
    if overrides.keys().any(|path| !paths.contains(path)) {
        return Err("sort override is not a configured sort field".into());
    }
    Ok(())
}

pub(crate) fn order_configuration(
    path: &FieldPath,
    schema: &Schema,
    locale: &str,
    defaults: SortOptions,
    overrides: &HashMap<FieldPath, SortConfig>,
) -> Result<(SortConfig, LogicalType, CollatorBorrowed<'static>), String> {
    let config = overrides.get(path).cloned().unwrap_or_else(|| SortConfig {
        locale: locale.into(),
        options: defaults,
    });
    let collator = make_collator(&config)?;
    let field = schema.field(path)?;
    if field.multi || matches!(field.kind, LogicalType::Object | LogicalType::Array(_)) {
        return Err(format!(
            "array/object sort needs explicit reduction: {}",
            path.display()
        ));
    }
    Ok((config, field.kind, collator))
}

pub(crate) fn composite_orders(
    schema: &Schema,
    mut has_column: impl FnMut(&FieldPath) -> bool,
) -> Result<Vec<Vec<CompositeSortPart>>, String> {
    schema
        .composite_sorts
        .iter()
        .map(|fields| {
            let parts = fields
                .iter()
                .map(simple_composite_part)
                .collect::<Option<Vec<_>>>();
            parts
                .filter(|parts| parts.len() >= 2 && parts.iter().all(|part| has_column(&part.path)))
                .ok_or_else(|| "composite sort requires configured resident scalar fields".into())
        })
        .collect()
}
