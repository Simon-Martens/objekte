//! Text representations and derived typed values.

use super::*;


/// Field-level binding inputs, resolved once from a schema and source rule.
pub struct BindingContext<'a> {
    pub collection: &'a str,
    pub path: &'a str,
    pub format: &'a str,
    pub schema: Option<&'a Value>,
}


/// A text view of one logical source value. The returned slice is borrowed
/// from `value` or reusable `output`; it cannot escape the current call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextPreparation {
    /// The value remains source text; an operator may prepare it for its mode.
    Stored,
    /// Visible text has already had markup, punctuation and spacing prepared.
    VisibleNormalized,
}


/// A promise that a representation is byte-for-byte equivalent to one of the
/// engine's existing text projections for candidate filtering. An incorrect
/// declaration can omit matches; implementations must verify the equivalence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTextProjection {
    Stored,
    VisibleNormalized(crate::TextFormat),
}


/// Produce one logical text value, with its preparation contract stated once.
pub trait TextRepresentation: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    fn binds(&self, context: &BindingContext<'_>) -> bool;
    /// Query-facing definitions may opt into an interpretation of a logical
    /// string beyond the field's legacy default format. Legacy low-level
    /// bindings continue to use `binds`.
    fn binds_explicit(&self, context: &BindingContext<'_>) -> bool {
        self.binds(context)
    }

    fn preparation(&self) -> TextPreparation {
        TextPreparation::Stored
    }

    /// Declare compatibility with an existing engine-owned index projection.
    /// The default keeps independent representations on exact scan fallback.
    fn native_index_projection(&self) -> Option<NativeTextProjection> {
        None
    }

    fn render<'a>(&self, value: &'a str, output: &'a mut String) -> Result<Option<&'a str>>;
}


/// Read-only access to logical leaves for a derived-value extractor.
pub trait RecordAccess {
    /// Whether the path reaches a value, including an empty array or object.
    fn path_present(&self, path: &JsonPath) -> Result<bool>;

    /// Visit scalar leaves in source order. A present empty container visits
    /// zero leaves; distinguish that case with `path_present`.
    fn visit_path(
        &self,
        path: &JsonPath,
        visit: &mut dyn FnMut(ScalarValue<'_>) -> Result<()>,
    ) -> Result<usize>;
}


impl RecordAccess for Value {
    fn path_present(&self, path: &JsonPath) -> Result<bool> {
        Ok(!crate::json_filter::values(self, path, false).is_empty())
    }

    fn visit_path(
        &self,
        path: &JsonPath,
        visit: &mut dyn FnMut(ScalarValue<'_>) -> Result<()>,
    ) -> Result<usize> {
        let mut count = 0;
        let mut error = None;
        crate::json_filter::visit_values(self, path, true, &mut |value| {
            let result = match value {
                Value::Null => visit(ScalarValue::Null),
                Value::Bool(value) => visit(ScalarValue::Bool(*value)),
                Value::Number(value) => {
                    // Sonic canonicalizes number spellings during the native
                    // projection. Use the same spelling for direct access.
                    sonic_rs::from_str::<SonicValue>(value.as_str())
                        .map_err(|_| Error::Request("invalid extracted JSON number".into()))
                        .and_then(|canonical| {
                            let number = canonical.as_raw_number().expect("parsed number");
                            visit(ScalarValue::Number(number.as_str()))
                        })
                }
                Value::String(value) => visit(ScalarValue::Text(value)),
                Value::Array(_) | Value::Object(_) => return true,
            };
            count += 1;
            if let Err(failure) = result {
                error = Some(failure);
                return false;
            }
            true
        });
        if let Some(error) = error {
            Err(error)
        } else {
            Ok(count)
        }
    }
}


impl RecordAccess for SonicValue {
    fn path_present(&self, path: &JsonPath) -> Result<bool> {
        Ok(!crate::json_filter::sonic_values(self, path, false).is_empty())
    }

    fn visit_path(
        &self,
        path: &JsonPath,
        visit: &mut dyn FnMut(ScalarValue<'_>) -> Result<()>,
    ) -> Result<usize> {
        let mut count = 0;
        for value in crate::json_filter::sonic_values(self, path, true) {
            if let Some(number) = value.as_raw_number() {
                // Match serde_json's arbitrary-precision spelling used by
                // direct verification before lending the temporary number.
                let canonical: serde_json::Number = number
                    .as_str()
                    .parse()
                    .map_err(|_| Error::Request("invalid extracted JSON number".into()))?;
                visit(ScalarValue::Number(canonical.as_str()))?;
                count += 1;
                continue;
            }
            let scalar = match value.as_ref() {
                ValueRef::Null => Some(ScalarValue::Null),
                ValueRef::Bool(value) => Some(ScalarValue::Bool(value)),
                ValueRef::String(value) => Some(ScalarValue::Text(value)),
                ValueRef::Number(_) => unreachable!("raw numbers enabled"),
                ValueRef::Array(_) | ValueRef::Object(_) => None,
            };
            if let Some(scalar) = scalar {
                visit(scalar)?;
                count += 1;
            }
        }
        Ok(count)
    }
}


/// Typed values preserve exact decimal spellings; `Missing` is represented by
/// `Extraction::Missing`, while a present null is `TypedValue::Null`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypedValue {
    Null,
    Bool(bool),
    Number(String),
    Text(String),
}


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Extraction {
    Missing,
    Present,
}


/// Append zero or more values to a worker-reused buffer. Errors are distinct
/// from missing and from a present field with no emitted values.
pub trait ValueExtractor: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    fn binds(&self, context: &BindingContext<'_>) -> bool;
    fn extract(
        &self,
        record: &dyn RecordAccess,
        output: &mut Vec<TypedValue>,
    ) -> Result<Extraction>;
}


/// Internal posting paths cannot alias a stored JSON field in public queries.
pub(crate) const DERIVED_PATH_PREFIX: &str = "\0objekte:derived";

pub(crate) fn is_derived_path(path: &JsonPath) -> bool {
    matches!(path.first(), Some(JsonPathStep::Key(name)) if name == DERIVED_PATH_PREFIX)
}


#[derive(Clone)]
pub(crate) struct BoundValueExtractor {
    pub extractor: Arc<dyn ValueExtractor>,
    pub path: JsonPath,
    pub identity: String,
    pub cacheable: bool,
}
