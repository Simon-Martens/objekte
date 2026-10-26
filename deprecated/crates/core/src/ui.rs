//! Presentation inheritance. Objects merge by option; arrays and scalar values replace.
use crate::{Editor, Result};
use serde_json::{Value, json};

pub fn inherit(parent: &Value, local: &Value) -> Value {
    let Some(options) = local.as_object() else {
        return if local.is_null() {
            parent.clone()
        } else {
            local.clone()
        };
    };
    let mut result = if options.get("inherit") == Some(&Value::Bool(false)) {
        json!({})
    } else if parent.is_object() {
        parent.clone()
    } else {
        json!({})
    };
    for (key, value) in options {
        if key == "inherit" {
            result[key] = value.clone();
            continue;
        }
        // Only option objects merge; explicit null, arrays and scalars replace.
        result[key] = if value.is_object() {
            inherit(&result[key], value)
        } else {
            value.clone()
        };
    }
    result
}

impl Editor {
    /// A child list inherits its target collection, then its parent's shared and local list options.
    pub fn child_list_ui(&self, parent: &str, field: &str) -> Result<Value> {
        let child = self.child_collection(parent, field)?;
        let parent_ui = &self.config().collection(parent)?.ui;
        let base = &self.config().collection(&child.collection)?.ui;
        let shared = inherit(base, &parent_ui["childLists"]["defaults"]);
        Ok(inherit(&shared, &parent_ui["childLists"]["fields"][field]))
    }

    pub fn document_ui(&self, name: &str) -> Result<Value> {
        self.document_schema(name)?;
        let ui = &self.config().ui;
        Ok(inherit(&ui["defaults"], &ui["documents"][name]))
    }

    pub fn document_editor_schema(&self, name: &str) -> Result<Value> {
        let mut schema = self.document_schema(name)?.clone();
        let ui = self.document_ui(name)?;
        if let Some(fields) = ui.get("fields") {
            crate::project_config::apply_fields(&mut schema, fields)?;
        }
        if schema.is_object() {
            schema["x-presentation"] = inherit(&ui["presentation"], &schema["x-presentation"]);
            schema["x-collection"] = json!({"fieldOrder": ui["fieldOrder"]});
        }
        Ok(schema)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_options_replace_only_their_option_and_reset_is_scoped() {
        let parent = json!({"list":{"columns":["a"],"defaultSort":{"field":"a","direction":"asc"}},"pageSize":30});
        let merged = inherit(
            &parent,
            &json!({"list":{"defaultSort":{"direction":"desc"}}}),
        );
        assert_eq!(merged["list"]["columns"], json!(["a"]));
        assert_eq!(
            merged["list"]["defaultSort"],
            json!({"field":"a","direction":"desc"})
        );
        let reset = inherit(&parent, &json!({"list":{"inherit":false,"columns":["b"]}}));
        assert_eq!(reset["list"], json!({"inherit":false,"columns":["b"]}));
        assert_eq!(reset["pageSize"], 30);
        assert_eq!(
            inherit(&parent, &json!({"inherit":false,"pageSize":5})),
            json!({"inherit":false,"pageSize":5})
        );
        assert_eq!(inherit(&parent, &json!({"list":null}))["list"], Value::Null);
    }
}
