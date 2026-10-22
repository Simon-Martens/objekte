use super::*;
use structured_editor_core::draft::InputKind;
pub struct Array;
#[derive(Template)]
#[template(path = "fields/array.html")]
struct Input {
    items: Vec<Item>,
    add: String,
}
struct Item {
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
                Ok(Item {
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
