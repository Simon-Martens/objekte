//! Stable entry URLs and explicit lifetime for private, tab-owned drafts.
use crate::{
    AppState,
    auth::Actor,
    error::{Result, WebError},
    routes,
    sessions::EditSession,
    views,
};
use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use std::collections::HashMap;

pub fn entry(
    actor: &Actor,
    url: &str,
    action: &str,
    file: Option<&str>,
    node: &str,
    expired: bool,
) -> Result<String> {
    let mut body = format!(
        "<form method=\"post\" action=\"{}\" data-draft-entry data-expired=\"{}\" data-on:submit__prevent=\"@post(el.getAttribute('action'),{{contentType:'form',retry:'never',retryMaxCount:0}})\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"node\" value=\"{}\"><input type=\"hidden\" name=\"draft\" value=\"\">",
        views::escape(action),
        expired,
        views::escape(&actor.csrf),
        views::escape(node)
    );
    if let Some(file) = file {
        body += &format!(
            "<input type=\"hidden\" name=\"file\" value=\"{}\">",
            views::escape(file)
        );
    }
    if expired {
        body += "<p role=\"alert\">This draft expired or is no longer available. Its unsaved server-side changes are no longer available. Reopen the latest saved record to continue.</p>";
    }
    body += if expired {
        "<button class=\"primary\">Reopen editor</button></form>"
    } else {
        "<p>Opening editor…</p><button>Open editor</button></form>"
    };
    views::panel(url, "Edit", &body)
}

pub fn resume(
    state: &AppState,
    actor: &Actor,
    form: &HashMap<String, String>,
    expected: &str,
) -> Result<Option<EditSession>> {
    let Some(id) = form.get("draft").filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    let gate = state.sessions.lock().unwrap().gate(id, actor)?;
    let _guard = gate.lock().unwrap();
    let mut session = state.sessions.lock().unwrap().get(id, actor)?.clone();
    if session.edit_key() != expected {
        return Err(WebError(
            StatusCode::NOT_FOUND,
            "Draft does not belong to this editor".into(),
        ));
    }
    if let Some(raw) = form.get("restore").filter(|raw| !raw.is_empty()) {
        let restored: HashMap<String, String> = serde_json::from_str(raw)
            .map_err(|error| WebError::bad(format!("Invalid draft recovery data: {error}")))?;
        if restored.get("version").and_then(|v| v.parse::<u64>().ok()) == Some(session.version) {
            let before = session.snapshot();
            session.apply(&restored)?;
            session.history.record(before, &session.snapshot());
        }
    }
    if let Some(node) = form.get("node") {
        if node.is_empty()
            || session
                .draft
                .at_path(node)
                .is_some_and(crate::tree::navigable)
        {
            session.view_path = node.clone();
        }
    }
    state
        .sessions
        .lock()
        .unwrap()
        .update(actor, session.clone())?;
    Ok(Some(session))
}

pub fn closed(
    state: &AppState,
    actor: &Actor,
    headers: &HeaderMap,
    session: &EditSession,
) -> Result<Response> {
    state.sessions.lock().unwrap().remove(&session.id, actor)?;
    let url = session.close_url();
    if !routes::fragment(headers) {
        return Ok(Redirect::to(&url).into_response());
    }
    let body = format!(
        "<main id=\"workspace\" data-closed-draft=\"{}\" data-navigate-to=\"{}\"><p>Editor closed.</p><a data-nav href=\"{}\">Return to record</a></main>",
        views::escape(&session.id),
        views::escape(&url),
        views::escape(&url)
    );
    routes::page(state, actor, headers, body)
}

pub fn expired(
    state: &AppState,
    actor: &Actor,
    headers: &HeaderMap,
    form: &HashMap<String, String>,
) -> Result<Response> {
    let url = form
        .get("edit_url")
        .filter(|url| url.starts_with("/collections/") || url.starts_with("/documents/"));
    let link = url
        .map(|url| {
            format!(
                "<a class=\"button\" data-nav href=\"{}\">Reopen editor</a>",
                views::escape(url)
            )
        })
        .unwrap_or_else(|| "<a data-nav href=\"/\">Return to repository</a>".into());
    let html = views::panel(
        url.map(String::as_str).unwrap_or("/"),
        "Draft expired",
        &format!(
            "<p role=\"alert\">This draft expired or is no longer available. Its unsaved server-side changes are no longer available. Reopen the latest saved record to continue.</p>{link}"
        ),
    )?;
    routes::page(
        state,
        actor,
        headers,
        html.replace(
            "id=\"workspace\"",
            "id=\"workspace\" data-expired-draft=\"true\"",
        ),
    )
}
