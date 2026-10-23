//! Feedback fragments are separate from input rendering so typing never replaces a control.
use super::escape;
use crate::sessions::EditSession;
use structured_editor_core::{ValidationIssue, draft::DraftField};

fn field_errors(id: &str, path: &str, issues: &[ValidationIssue]) -> String {
    let mut html = format!("<div id=\"{id}\" class=\"field-errors\" aria-live=\"polite\">");
    for issue in issues.iter().filter(|issue| issue.path == path) {
        html += &format!("<p>{}</p>", escape(&issue.message));
    }
    html + "</div>"
}
fn field_feedback(
    field: &DraftField,
    path: &str,
    issues: &[ValidationIssue],
    output: &mut Vec<String>,
    focus: &str,
) {
    if field.hidden || !field.present {
        return;
    }
    output.push(field_errors(&format!("errors-{}", field.id), path, issues));
    if crate::tree::navigable(field) && field.id != focus {
        return;
    }
    for item in &field.items {
        field_feedback(
            item,
            &structured_editor_core::draft::child_path(path, &item.key),
            issues,
            output,
            focus,
        );
    }
}
pub fn fragments(session: &EditSession, issues: &[ValidationIssue]) -> Vec<String> {
    let mut output = Vec::new();
    if session.raw.is_none() {
        let selected = session
            .draft
            .at_path(&session.view_path)
            .filter(|f| crate::tree::navigable(f));
        let fields: Vec<_> = selected
            .map(|f| vec![f])
            .unwrap_or_else(|| session.draft.fields.iter().collect());
        for field in fields {
            field_feedback(
                field,
                &if selected.is_some() {
                    session.view_path.clone()
                } else if session.draft.root_value {
                    String::new()
                } else {
                    format!("/{}", field.key.replace('~', "~0").replace('/', "~1"))
                },
                issues,
                &mut output,
                selected.map(|f| f.id.as_str()).unwrap_or(""),
            );
        }
    }
    output.push(field_errors("record-id-errors", "/id", issues));
    let mut summary = String::from(
        "<div id=\"validation-summary\" class=\"validation-summary\" role=\"status\" aria-live=\"polite\">",
    );
    if issues.is_empty() {
        summary += "<p class=\"valid\"><i class=\"ri-checkbox-circle-line\" aria-hidden=\"true\"></i>All fields are valid</p><p class=\"help\">Changes have not been saved.</p>";
    } else {
        summary += &format!(
            "<p class=\"invalid-text\">{} issue(s) to resolve</p><ul>",
            issues.len()
        );
        for issue in issues {
            summary += &format!(
                "<li><code>{}</code>{}</li>",
                escape(&issue.path),
                escape(&issue.message)
            );
        }
        summary += "</ul>";
    }
    output.push(summary + "</div>");
    output
}
