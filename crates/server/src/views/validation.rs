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
) {
    if field.hidden || !field.present {
        return;
    }
    output.push(field_errors(&format!("errors-{}", field.id), path, issues));
    for (index, item) in field.items.iter().enumerate() {
        field_feedback(item, &format!("{path}/{index}"), issues, output);
    }
}
pub fn fragments(session: &EditSession, issues: &[ValidationIssue]) -> Vec<String> {
    let mut output = Vec::new();
    if session.raw.is_none() {
        for field in &session.draft.fields {
            field_feedback(
                field,
                &format!("/{}", field.key.replace('~', "~0").replace('/', "~1")),
                issues,
                &mut output,
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
