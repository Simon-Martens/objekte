//! Deletion review keeps reference protection visible and placements actionable.
use crate::templates::Template;
use crate::{
    AppState,
    auth::Actor,
    blocking,
    error::{Result, WebError},
    routes::{fragment, mutation_actor, page, page_actor, stream},
    views,
};
use axum::{
    Router,
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use std::{collections::HashMap, sync::Arc};
use structured_editor_core::Editor;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/collections/{name}/records/{id}/delete",
            get(review).post(delete),
        )
        .route(
            "/collections/{name}/records/{id}/delete/unlink",
            post(unlink),
        )
}

#[derive(serde::Serialize)]
struct Placement {
    collection: String,
    id: String,
    field: String,
    label: String,
    list_label: String,
    context: String,
    url: String,
    revision: String,
    editable: bool,
}
#[derive(serde::Serialize)]
struct Reference {
    label: String,
    path: String,
    url: String,
}
crate::templates::template!(DeleteReview<'_>, "delete.html");
#[derive(serde::Serialize)]
struct DeleteReview<'a> {
    actor: crate::templates::ActorContext<'a>,
    title: String,
    back: String,
    revision: String,
    placements: Vec<Placement>,
    references: Vec<Reference>,
    blocked: bool,
    message: &'a str,
}

fn render(editor: &Editor, actor: &Actor, name: &str, id: &str, message: &str) -> Result<String> {
    editor.require_operation(name, "delete")?;
    editor.require_operation(name, "read")?;
    if !actor.editable() {
        return Err(WebError(StatusCode::FORBIDDEN, "Read-only account".into()));
    }
    let doc = editor.get(name, id)?;
    let parents = editor.parent_links(name, id)?;
    let incoming = editor.references_to(name, id)?;
    let mut placements = Vec::new();
    for parent in &parents {
        placements.push(Placement {
            collection: parent.collection.clone(),
            id: parent.id.clone(),
            field: parent.field.clone(),
            label: parent.label.clone(),
            list_label: editor
                .child_collection(&parent.collection, &parent.field)?
                .label,
            context: editor
                .parent_links(&parent.collection, &parent.id)?
                .iter()
                .map(|ancestor| ancestor.label.as_str())
                .collect::<Vec<_>>()
                .join(" · "),
            url: views::record_url(&parent.collection, &parent.id),
            revision: editor.get(&parent.collection, &parent.id)?.revision,
            editable: editor.allows(&parent.collection, "update"),
        });
    }
    let mut references = Vec::new();
    for reference in &incoming {
        if parents.iter().any(|p| {
            p.collection == reference.collection
                && p.id == reference.record_id
                && reference.path.starts_with(&format!(
                    "/{}/",
                    p.field.replace('~', "~0").replace('/', "~1")
                ))
        }) {
            continue;
        }
        let readable = editor.allows(&reference.collection, "read");
        references.push(Reference {
            label: if readable {
                let source = editor.get(&reference.collection, &reference.record_id)?;
                editor.label(
                    &reference.collection,
                    &source.value,
                    &reference.record_id,
                    "reference",
                )?
            } else {
                "A record in a restricted collection".into()
            },
            path: if readable {
                reference.path.clone()
            } else {
                String::new()
            },
            url: if readable {
                views::record_url(&reference.collection, &reference.record_id)
            } else {
                String::new()
            },
        });
    }
    let back = views::record_url(name, id);
    let modal = DeleteReview {
        actor: actor.into(),
        title: editor.label(name, &doc.value, id, "record")?,
        back: back.clone(),
        revision: doc.revision,
        placements,
        references,
        blocked: !incoming.is_empty(),
        message,
    }
    .render()?;
    let mut html = views::record(editor, name, id, actor)?;
    html = html.replacen(
        &format!("data-url=\"{back}\""),
        &format!("data-url=\"{back}/delete\""),
        1,
    );
    html.insert_str(html.rfind("</main>").unwrap(), &modal);
    Ok(html)
}

async fn review(
    State(state): State<Arc<AppState>>,
    Path((name, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response> {
    let actor = match page_actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(response) => return Ok(response),
    };
    blocking(move || {
        page(
            &state,
            &actor,
            &headers,
            render(&state.editor, &actor, &name, &id, "")?,
        )
    })
    .await
}

fn failure(
    state: &AppState,
    actor: &Actor,
    headers: &HeaderMap,
    name: &str,
    id: &str,
    error: WebError,
) -> Result<Response> {
    let mut response = page(
        state,
        actor,
        headers,
        render(&state.editor, actor, name, id, &error.1)?,
    )?;
    if fragment(headers) {
        response
            .headers_mut()
            .insert("x-editor-error-status", error.0.as_str().parse().unwrap());
    } else {
        *response.status_mut() = error.0;
    }
    Ok(response)
}

async fn unlink(
    State(state): State<Arc<AppState>>,
    Path((name, id)): Path<(String, String)>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response> {
    let actor = mutation_actor(&state, &headers, &form).await?;
    blocking(move || {
        state.editor.require_operation(&name, "delete")?;
        state.editor.require_operation(&name, "read")?;
        state.editor.get(&name, &id)?;
        let field = |key| {
            form.get(key)
                .ok_or_else(|| WebError::bad(format!("Missing {key}")))
        };
        let parent = field("parent_collection")?;
        let parent_id = field("parent_id")?;
        let parent_field = field("parent_field")?;
        state.editor.require_operation(parent, "read")?;
        state.editor.require_operation(parent, "update")?;
        if state
            .editor
            .child_collection(parent, parent_field)?
            .collection
            != name
        {
            return Err(WebError::bad("This list does not contain this collection."));
        }
        let message = match state.editor.set_child_membership(
            parent,
            parent_id,
            parent_field,
            &id,
            false,
            field("revision")?,
        ) {
            Ok(_) => "Removed from the list. The record still exists until you delete it.",
            Err(error) => {
                return failure(&state, &actor, &headers, &name, &id, WebError::from(error));
            }
        };
        page(
            &state,
            &actor,
            &headers,
            render(&state.editor, &actor, &name, &id, message)?,
        )
    })
    .await
}

async fn delete(
    State(state): State<Arc<AppState>>,
    Path((name, id)): Path<(String, String)>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response> {
    let actor = mutation_actor(&state, &headers, &form).await?;
    let revision = form
        .get("revision")
        .cloned()
        .ok_or_else(|| WebError::bad("Revision required"))?;
    blocking(move || {
        if let Err(error) = state.editor.delete(&name, &id, &revision) {
            let error = WebError::from(error);
            if error.0 != StatusCode::CONFLICT {
                return Err(error);
            }
            return failure(&state, &actor, &headers, &name, &id, error);
        }
        let html = views::collection(&state.editor, &name, "", 0, &actor)?;
        if fragment(&headers) {
            Ok(stream(vec![
                views::navigation(&state.editor, &html)?,
                html,
                views::notice("Record deleted."),
            ]))
        } else {
            Ok(Redirect::to(&format!("/collections/{}", views::encode(&name))).into_response())
        }
    })
    .await
}
