use super::*;
use structured_editor_core::draft::InputKind;
pub struct Array;
crate::templates::template!(Input, "fields/array.html");
#[derive(serde::Serialize)]
struct Input {
    items: Vec<Item>,
    add: String,
}
#[derive(serde::Serialize)]
struct Item {
    linked: bool,
    delete_url: String,
    delete_label: String,
    html: String,
    remove: String,
    up: String,
    down: String,
    first: bool,
    last: bool,
}
impl Control for Array {
    fn accepts(&self, field: &DraftField) -> bool {
        field.kind == InputKind::Array
    }
    fn render(
        &self,
        field: &DraftField,
        path: &str,
        context: &Context<'_>,
        registry: &Registry,
    ) -> Result<String> {
        let base = format!(
            "/edit-sessions/{}/fields/{}/actions",
            context.session, field.id
        );
        let items = field
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let target = item
                    .schema
                    .pointer("/x-relation/collection")
                    .and_then(serde_json::Value::as_str);
                let linked = target.is_some();
                let target_id = if item.kind == InputKind::Text {
                    item.raw.clone()
                } else {
                    serde_json::from_str::<serde_json::Value>(&item.raw)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default()
                };
                let deletable = target.filter(|target| {
                    linked
                        && context.editor.allows(target, "read")
                        && context.editor.allows(target, "delete")
                        && context.editor.get(target, &target_id).is_ok()
                });
                Ok(Item {
                    linked,
                    delete_url: deletable
                        .map(|target| {
                            format!("{}/delete", crate::views::record_url(target, &target_id))
                        })
                        .unwrap_or_default(),
                    delete_label: deletable
                        .map(|target| crate::views::record_kind(context.editor, target))
                        .transpose()?
                        .map(|kind| format!("Delete {kind}…"))
                        .unwrap_or_default(),
                    html: registry.render(item, &format!("{path}/{index}"), context, true)?,
                    remove: format!("{base}/remove?item={}", item.id),
                    up: format!("{base}/up?item={}", item.id),
                    down: format!("{base}/down?item={}", item.id),
                    first: index == 0,
                    last: index + 1 == field.items.len(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Input {
            items,
            add: format!("{base}/add"),
        }
        .render()?)
    }
}
