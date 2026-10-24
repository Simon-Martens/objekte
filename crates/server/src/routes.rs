use crate::{
    AppState,
    auth::Actor,
    blocking,
    error::{Result, WebError},
    views,
};
use axum::{
    Router,
    extract::{DefaultBodyLimit, Form, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use std::{collections::HashMap, sync::Arc};
use tower_http::services::ServeDir;

type App = State<Arc<AppState>>;
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(crate::media::router())
        .merge(crate::documents::router())
        .merge(crate::accounts::router())
        .merge(crate::hierarchy::router())
        .merge(crate::deletion::router())
        .route("/", get(index))
        .route("/health", get(|| async { "ok" }))
        .route("/auth/login", get(login_page).post(login))
        .route("/auth/logout", post(logout))
        .route("/collections/{name}", get(collection))
        .route("/collections/{name}/records/{id}", get(record))
        .route("/collections/{name}/records/{id}/export", get(export))
        .route("/collections/{name}/edit-sessions", post(create))
        .route("/collections/{name}/records/{id}/edit", get(edit_entry))
        .route("/collections/{name}/new", get(new_entry))
        .route(
            "/collections/{name}/records/{id}/edit-sessions",
            post(start),
        )
        .route("/edit-sessions/{id}", get(edit))
        .route("/edit-sessions/{id}/live-validation", post(live_validation))
        .route("/edit-sessions/{id}/{action}", post(command))
        .route(
            "/edit-sessions/{id}/fields/{field}/suggestions",
            post(suggestions),
        )
        .route(
            "/edit-sessions/{id}/fields/{field}/actions/{action}",
            post(array_command),
        )
        .nest_service(
            "/assets",
            ServeDir::new(&state.assets.root).append_index_html_on_directories(false),
        )
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .layer(axum::middleware::from_fn(response_headers))
        .with_state(state)
}
async fn response_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let partial = fragment(request.headers());
    let mut response = next.run(request).await;
    if partial && (response.status().is_client_error() || response.status().is_server_error()) {
        let status = response.status();
        let message = response
            .extensions_mut()
            .remove::<crate::error::ErrorNotice>()
            .map(|notice| notice.0)
            .unwrap_or_else(|| match status {
                StatusCode::PAYLOAD_TOO_LARGE => {
                    "This upload is too large for the server's request limit.".into()
                }
                StatusCode::NOT_FOUND => {
                    "This page or action is no longer available. Refresh the page and try again."
                        .into()
                }
                _ => format!(
                    "The request could not be completed ({}). Refresh the page and try again.",
                    status.as_str()
                ),
            });
        response = stream(vec![format!(
            "<div id=\"notification-region\" role=\"alert\" aria-live=\"assertive\">{}</div>",
            views::escape(&message)
        )]);
        response
            .headers_mut()
            .insert("x-editor-error-status", status.as_str().parse().unwrap());
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert("x-frame-options", "DENY".parse().unwrap());
    response
        .headers_mut()
        .insert("referrer-policy", "same-origin".parse().unwrap());
    response
}
pub(crate) fn fragment(headers: &HeaderMap) -> bool {
    headers
        .get("datastar-request")
        .and_then(|v| v.to_str().ok())
        == Some("true")
}
pub(crate) fn stream(fragments: Vec<String>) -> Response {
    stream_mode(fragments, "outer")
}
fn stream_mode(fragments: Vec<String>, mode: &str) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CACHE_CONTROL, "no-store"),
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        views::patches_mode(&fragments, mode),
    )
        .into_response()
}
pub(crate) fn page(
    state: &AppState,
    actor: &Actor,
    headers: &HeaderMap,
    html: String,
) -> Result<Response> {
    if fragment(headers) {
        Ok(stream(vec![
            crate::preferences::fragment(&state.editor),
            views::navigation(&state.editor, &html)?,
            html,
            views::notice(""),
        ]))
    } else {
        Ok(Html(views::shell(&state.editor, actor, &html, &state.assets)?).into_response())
    }
}
pub(crate) async fn page_actor(
    state: &AppState,
    headers: &HeaderMap,
) -> std::result::Result<Actor, Response> {
    match state.auth.actor(headers).await {
        Ok(actor) => Ok(actor),
        Err(error) if error.0 == StatusCode::UNAUTHORIZED => {
            if fragment(headers) {
                Err(stream(vec!["<main id=\"workspace\" data-url=\"/auth/login\"><h1>Session expired</h1><a href=\"/auth/login\">Sign in again</a></main>".into()]))
            } else {
                Err(Redirect::to("/auth/login").into_response())
            }
        }
        Err(error) => Err(error.into_response()),
    }
}
async fn index(State(state): App, headers: HeaderMap) -> Result<Response> {
    let actor = match page_actor(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    blocking(move || {
        let html = if let Some(name) = state.editor.config().collections.keys().find(|n| state.editor.allows(n, "read")) {
            views::collection(&state.editor, name, "", 0, &actor)?
        } else if let Some(name) = state.editor.config().media_collections.keys().find(|n| state.editor.allows(n, "read")) {
            crate::media::render(&state, &actor, name, "", "", 0, "")?
        } else { views::panel("/", "Repository", "<p>No readable collections configured. Open editor settings or an assigned JSON document from the sidebar.</p>")? };
        page(&state, &actor, &headers, html)
    })
    .await
}
async fn login_page(State(state): App) -> Result<Html<String>> {
    Ok(Html(views::login(&state.assets, "")?))
}
#[derive(Deserialize)]
struct Login {
    email: String,
    password: String,
}
async fn login(
    State(state): App,
    headers: HeaderMap,
    Form(input): Form<Login>,
) -> Result<Response> {
    if headers.get("sec-fetch-site").and_then(|h| h.to_str().ok()) == Some("cross-site") {
        return Err(WebError(
            StatusCode::FORBIDDEN,
            "Cross-site login refused.".into(),
        ));
    }
    match state.auth.login(&input.email, input.password).await {
        Ok(token) => {
            let mut response = Redirect::to("/").into_response();
            response.headers_mut().insert(
                header::SET_COOKIE,
                state
                    .auth
                    .cookie(&token)
                    .parse()
                    .map_err(WebError::internal)?,
            );
            Ok(response)
        }
        Err(error) if error.0 == StatusCode::UNAUTHORIZED => Ok((
            StatusCode::UNAUTHORIZED,
            Html(views::login(
                &state.assets,
                "Email or password was not recognized.",
            )?),
        )
            .into_response()),
        Err(e) => Err(e),
    }
}
async fn logout(
    State(state): App,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response> {
    let actor = state.auth.actor(&headers).await?;
    state.auth.mutation(
        &actor,
        form.get("csrf").map(String::as_str).unwrap_or(""),
        false,
    )?;
    state.auth.logout(&actor).await?;
    state.sessions.lock().unwrap().logout(&actor);
    let mut response = Redirect::to("/auth/login").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        state.auth.cookie("").parse().map_err(WebError::internal)?,
    );
    Ok(response)
}
async fn collection(
    State(state): App,
    Path(name): Path<String>,
    Query(mut query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response> {
    query.remove("datastar");
    let actor = match page_actor(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    blocking(move || {
        if headers
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("no-cache") || v.contains("max-age=0"))
        {
            state.editor.refresh_read_cache();
        }
        let html = views::list::render(&state.editor, &name, &query, &actor)?;
        page(&state, &actor, &headers, html)
    })
    .await
}
async fn record(
    State(state): App,
    Path((name, id)): Path<(String, String)>,
    Query(mut query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response> {
    let actor = match page_actor(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    query.remove("datastar");
    blocking(move || {
        let html = views::record_at(&state.editor, &name, &id, &actor, &query)?;
        page(&state, &actor, &headers, html)
    })
    .await
}
pub(crate) async fn mutation_actor(
    state: &AppState,
    headers: &HeaderMap,
    form: &HashMap<String, String>,
) -> Result<Actor> {
    let actor = state.auth.actor(headers).await?;
    state.auth.mutation(
        &actor,
        form.get("csrf").map(String::as_str).unwrap_or(""),
        true,
    )?;
    Ok(actor)
}
async fn edit_entry(
    State(state): App,
    Path((name, id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response> {
    entry_record(state, name, Some(id), query, headers).await
}
async fn new_entry(
    State(state): App,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response> {
    entry_record(state, name, None, query, headers).await
}
async fn entry_record(
    state: Arc<AppState>,
    name: String,
    id: Option<String>,
    query: HashMap<String, String>,
    headers: HeaderMap,
) -> Result<Response> {
    let actor = match page_actor(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    if !actor.editable() {
        return Err(WebError(StatusCode::FORBIDDEN, "Read-only account.".into()));
    }
    blocking(move || {
        state
            .editor
            .require_operation(&name, if id.is_some() { "update" } else { "create" })?;
        let base = id
            .as_ref()
            .map(|id| views::record_url(&name, id))
            .unwrap_or_else(|| format!("/collections/{}", views::encode(&name)));
        let mut url = format!("{base}/{}", if id.is_some() { "edit" } else { "new" });
        let node = query.get("node").map(String::as_str).unwrap_or("");
        if !node.is_empty() {
            url += &format!("?node={}", views::encode(node));
        }
        let html = crate::draft_lifecycle::entry(
            &actor,
            &url,
            &format!("{base}/edit-sessions"),
            None,
            node,
            false,
        )?;
        page(&state, &actor, &headers, html)
    })
    .await
}
async fn start(
    State(state): App,
    Path((name, id)): Path<(String, String)>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response> {
    start_edit(state, name, Some(id), headers, form).await
}
async fn create(
    State(state): App,
    Path(name): Path<String>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response> {
    start_edit(state, name, None, headers, form).await
}
async fn start_edit(
    state: Arc<AppState>,
    name: String,
    id: Option<String>,
    headers: HeaderMap,
    form: HashMap<String, String>,
) -> Result<Response> {
    let actor = mutation_actor(&state, &headers, &form).await?;
    blocking(move || {
        let base = id
            .as_ref()
            .map(|id| views::record_url(&name, id))
            .unwrap_or_else(|| format!("/collections/{}", views::encode(&name)));
        let key = format!("{base}/{}", if id.is_some() { "edit" } else { "new" });
        match crate::draft_lifecycle::resume(&state, &actor, &form, &key) {
            Ok(Some(session)) => {
                return page(
                    &state,
                    &actor,
                    &headers,
                    views::edit(&state.editor, &session, &actor, &state.fields, &[], "")?,
                );
            }
            Err(error) if error.0 == StatusCode::NOT_FOUND => {
                let html = crate::draft_lifecycle::entry(
                    &actor,
                    &key,
                    &format!("{base}/edit-sessions"),
                    None,
                    form.get("node").map(String::as_str).unwrap_or(""),
                    true,
                )?;
                return page(&state, &actor, &headers, html);
            }
            Err(error) => return Err(error),
            Ok(None) => {}
        }
        let context_count = [
            "parent_collection",
            "parent_id",
            "parent_field",
            "parent_revision",
        ]
        .iter()
        .filter(|k| form.contains_key(**k))
        .count();
        if context_count != 0 && (context_count != 4 || id.is_some()) {
            return Err(WebError::bad("Incomplete or invalid parent context"));
        }
        let mut session = crate::sessions::Sessions::default().start(
            &state.editor,
            &actor,
            &name,
            id.as_deref(),
        )?;
        state
            .sessions
            .lock()
            .unwrap()
            .register(&actor, session.clone())?;
        if let Some(path) = form.get("node") {
            if session
                .draft
                .at_path(path)
                .is_some_and(crate::tree::navigable)
            {
                session.view_path = path.clone();
                state.sessions.lock().unwrap().insert(session.clone());
            }
        }
        if id.is_none() {
            if let (Some(parent), Some(parent_id), Some(field), Some(revision)) = (
                form.get("parent_collection"),
                form.get("parent_id"),
                form.get("parent_field"),
                form.get("parent_revision"),
            ) {
                if state.editor.child_collection(parent, field)?.collection != name {
                    return Err(WebError::bad("Child collection mismatch"));
                }
                state.editor.require_operation(parent, "update")?;
                state.editor.require_operation(parent, "read")?;
                if state.editor.get(parent, parent_id)?.revision != *revision {
                    return Err(WebError(
                        axum::http::StatusCode::CONFLICT,
                        "Parent changed; reload its overview before adding a child".into(),
                    ));
                }
                session.parent = Some(crate::sessions::ParentContext {
                    collection: parent.clone(),
                    id: parent_id.clone(),
                    field: field.clone(),
                    revision: revision.clone(),
                });
                state.sessions.lock().unwrap().insert(session.clone());
            }
            if let Some(context) = form.get("context") {
                let path: Vec<String> =
                    serde_json::from_str(context).map_err(WebError::internal)?;
                session.draft = state
                    .editor
                    .create_draft(&name, &state.editor.contextual_values(&name, &path)?)?;
                state.sessions.lock().unwrap().insert(session.clone());
            }
        }

        session.mark_clean();
        state
            .sessions
            .lock()
            .unwrap()
            .update(&actor, session.clone())?;
        page(
            &state,
            &actor,
            &headers,
            views::edit(&state.editor, &session, &actor, &state.fields, &[], "")?,
        )
    })
    .await
}
async fn edit(State(state): App, Path(id): Path<String>, headers: HeaderMap) -> Result<Response> {
    let actor = match page_actor(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    if !actor.editable() {
        return Err(WebError(StatusCode::FORBIDDEN, "Read-only account.".into()));
    }
    blocking(move || {
        let session = state.sessions.lock().unwrap().get(&id, &actor)?.clone();
        let html = views::edit(&state.editor, &session, &actor, &state.fields, &[], "")?;
        page(&state, &actor, &headers, html)
    })
    .await
}
async fn command(
    State(state): App,
    Path((id, action)): Path<(String, String)>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response> {
    perform(state, id, action, None, None, headers, form).await
}
async fn array_command(
    State(state): App,
    Path((id, field, action)): Path<(String, String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response> {
    perform(
        state,
        id,
        action,
        Some(field),
        query.get("item").cloned(),
        headers,
        form,
    )
    .await
}
async fn suggestions(
    State(state): App,
    Path((id, field)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response> {
    let actor = mutation_actor(&state, &headers, &form).await?;
    blocking(move || {
        // Decode current input on a clone: a lookup never changes draft/version or disk.
        let mut session = state.sessions.lock().unwrap().get(&id, &actor)?.clone();
        session.apply(&form)?;
        let input = session
            .draft
            .field(&field)
            .filter(|f| !f.hidden && !f.read_only)
            .ok_or_else(|| WebError::bad("Unknown editable field"))?;
        let search = if query.get("all").is_some_and(|value| value == "1") { "" } else { input.raw.as_str() };
        let context = session.value().unwrap_or(serde_json::json!({}));
        let mut values = if session.document.is_some() {
            Vec::new()
        } else {
            state
                .editor
                .suggestions_query(&session.collection, &input.key, &context, search)?
        };
        if let Some(options) = input.schema["x-options"].as_array() {
            values.extend(
                options
                    .iter()
                    .filter_map(|v| v.as_str())
                    .filter(|s| s.to_lowercase().contains(&search.to_lowercase()))
                    .map(str::to_owned),
            );
        }
        values.sort_by(|a, b| structured_editor_core::alphabetic_cmp(a, b).then(a.cmp(b)));
        values.dedup();
        let mut html = format!("<datalist id=\"suggestions-{}\" data-query=\"{}\">", views::escape(&field), views::escape(search));
        for value in values {
            html += &format!("<option value=\"{}\"></option>", views::escape(&value));
        }
        html += "</datalist>";
        if fragment(&headers) {
            Ok(stream(vec![html]))
        } else {
            Ok(Html(html).into_response())
        }
    })
    .await
}

/// Validates a snapshot only; it never advances the stored draft or replaces inputs.
async fn live_validation(
    State(state): App,
    Path(id): Path<String>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response> {
    let actor = mutation_actor(&state, &headers, &form).await?;
    blocking(move || {
        let mut session = state.sessions.lock().unwrap().get(&id, &actor)?.clone();
        // A command may have completed while this advisory request was in flight.
        if form.get("version").and_then(|v| v.parse::<u64>().ok()) != Some(session.version) {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        session.apply(&form)?;
        let mut issues = session.validate(&state.editor)?;
        if session.document.is_none() && (session.proposed_id != session.record_id || session.revision.is_none()) {
            issues.extend(
                state
                    .editor
                    .validate_new_id(&session.collection, &session.proposed_id)?,
            );
        }
        issues.sort_by(|a, b| a.path.cmp(&b.path).then(a.message.cmp(&b.message)));
        issues.dedup_by(|a, b| a.path == b.path && a.message == b.message);
        let mut fragments = views::validation::fragments(&session, &issues);
        fragments.push(views::draft_exit(&session)?);
        for id in &session.preview {if let Some(field)=session.draft.field(id).filter(|f|f.present&&!f.hidden){fragments.push(format!("<section id=\"preview-{}\" class=\"markdown-preview\"><p class=\"eyebrow\">Live preview</p>{}</section>",field.id,views::markdown(&field.raw)));}}
        Ok(stream(fragments))
    })
    .await
}

async fn perform(
    state: Arc<AppState>,
    id: String,
    action: String,
    field: Option<String>,
    item: Option<String>,
    headers: HeaderMap,
    form: HashMap<String, String>,
) -> Result<Response> {
    let actor = mutation_actor(&state, &headers, &form).await?;
    blocking(move || {
        if state.sessions.lock().unwrap().get(&id,&actor).is_err() {
            return crate::draft_lifecycle::expired(&state,&actor,&headers,&form);
        }
        // Lock only the affected drafts. Related-save touches both parent and child;
        // consistent ID order prevents deadlocks between concurrent commands.
        let gates = {
            let mut registry = state.sessions.lock().unwrap();
            let mut ids = vec![id.clone()];
            if let Some((parent, _)) = &registry.get(&id, &actor)?.return_to {
                ids.push(parent.clone());
            }
            ids.sort();
            ids.dedup();
            let mut gates = Vec::new();
            for key in ids {
                match registry.gate(&key, &actor) {
                    Ok(gate) => gates.push(gate),
                    Err(error) if key == id => return Err(error),
                    Err(_) => {} // Expired parent does not prevent saving the child.
                }
            }
            gates
        };
        let _guards: Vec<_> = gates.iter().map(|gate| gate.lock().unwrap()).collect();
        let mut session = state.sessions.lock().unwrap().get(&id, &actor)?.clone();
        let before_input=session.snapshot();
        session.apply(&form)?;
        if action == "discard" || action == "discard-close" { return crate::draft_lifecycle::closed(&state,&actor,&headers,&session,None); }
        if action == "close" && (!session.dirty() || form.get("confirm_discard").is_some_and(|value| value == "false")) { return crate::draft_lifecycle::closed(&state,&actor,&headers,&session,None); }
        if action == "close" || action == "keep-editing" {
            session.close_requested = action == "close";
            session.history.record(before_input,&session.snapshot());
            state.sessions.lock().unwrap().update(&actor,session.clone())?;
            return page(&state,&actor,&headers,views::edit(&state.editor,&session,&actor,&state.fields,&[],"")?);
        }
        if action == "save-close" { session.close_requested = false; }
        session.history.record(before_input,&session.snapshot());
        let before_command=session.snapshot();
        let mut issues = Vec::new();
        let mut message = String::new();
        let mut message_kind = "error";
        if let Some(field) = field {
            if action == "browse" {
                if let Some((path, _)) = session.draft.paths().into_iter().find(|(_, f)| f.id == field && crate::tree::navigable(f)) { session.view_path = path; }
            } else if action == "create-related" {
                // Persist the parent before opening the child, including incomplete input.
                state.sessions.lock().unwrap().update(&actor, session.clone())?;
                let child = related_session(&state, &actor, &session, &field, &form);
                match child {
                    Ok(child) => return page(&state, &actor, &headers, views::edit(&state.editor, &child, &actor, &state.fields, &[], "")?),
                    Err(error) => message = error.1,
                }
            } else {
                let result = if action == "include" || action == "omit" {
                    if action == "include" { session.draft.include_field(&session.schema, &field) } else { session.draft.set_presence(&field, false) }.map_err(WebError::bad)
                } else if ["add", "remove", "up", "down"].contains(&action.as_str()) {
                    session.draft.array_action(&session.schema, &field, &action, item.as_deref()).map_err(WebError::bad)
                } else {
                    crate::pickers::action(&state.editor, &mut session, &field, &action, &form)
                };
                if let Err(error) = result { message = error.1; }
            }
        } else {
            if session.document.is_some() && ["generate-id", "check-id"].contains(&action.as_str()) { return Err(WebError::bad("Assigned documents have fixed paths")); }
            match action.as_str() {
                "browse-root" => session.view_path.clear(),
                "undo" => {if let Some(previous)=session.history.undo(session.snapshot()){session.restore(previous);}},
                "redo" => {if let Some(next)=session.history.redo(session.snapshot()){session.restore(next);message_kind="success";message="Change restored.".into();}},
                "raw" => match session.value() {
                    Ok(value) => session.raw = Some(serde_json::to_string_pretty(&value).unwrap()),
                    Err(errors) => issues = errors,
                },
                "form" => match session.value() {
                    Ok(value) if value.is_object() || session.document.is_some() => {
                        session.draft = if session.document.is_some() { structured_editor_core::draft::Draft::document_form(&session.schema, &value) } else { state.editor.draft_for_id(&session.collection, &session.proposed_id, &value)? };
                        session.raw = None;
                    },
                    Ok(_) => message = "A record must be a JSON object.".into(),
                    Err(errors) => issues = errors,
                },
                "save" | "save-close" => match session.save(&state.editor) {
                    Ok(updated_items) => {
                        message_kind = "success";
                        message = if let Some(count) = updated_items {
                            format!("ID changed. {count} referenced {} updated.", if count == 1 { "item" } else { "items" })
                        } else {
                            "Saved to disk.".into()
                        };
                        session.close_requested = false;
                        session.parent = None;
                        if let Some((parent_id, field)) = session.return_to.take() {
                            let parent = state.sessions.lock().unwrap().get(&parent_id, &actor).map(|s| s.clone());
                            if let Ok(mut parent) = parent {
                                parent.draft.set_field(&field, serde_json::json!(session.record_id)).map_err(WebError::bad)?;
                                parent.version += 1;
                                parent.picker = None;
                                parent.refresh_derived(&state.editor)?;
                                state.sessions.lock().unwrap().update(&actor, parent)?;
                                message = "Related record created and selected. Save the parent record to retain the reference.".into();
                            } else { message_kind = "warning"; message = "Saved. The parent draft has expired; select this record from a new draft.".into(); }
                        }
                        if action == "save-close" { return crate::draft_lifecycle::closed(&state,&actor,&headers,&session,Some(&message)); }
                    },
                    Err(structured_editor_core::Error::Validation(errors)) => issues = errors,
                    Err(error) => message = error.to_string(),
                },
                "validate" => match session.validate(&state.editor) {
                    Ok(errors) => { issues = errors; if issues.is_empty() { message_kind = "success"; message = "The draft is valid.".into(); } },
                    Err(error) => message = error.1,
                },
                "generate-id" => match session.value() {
                    Ok(value) => match state.editor.generate_id(&session.collection, &value) {
                        Ok(id) => session.proposed_id = id,
                        Err(error) => message = error.to_string(),
                    },
                    Err(errors) => issues = errors,
                },
                "check-id" => {
                    if session.proposed_id == session.record_id && session.revision.is_some() {
                        message_kind = "success"; message = "This is the current record ID.".into();
                    } else {
                        issues = state.editor.validate_new_id(&session.collection, &session.proposed_id)?;
                        if issues.is_empty() { message_kind = "success"; message = "This ID is available.".into(); }
                    }
                },
                _ => return Err(WebError::bad("Unknown operation")),
            }
        }
        session.refresh_derived(&state.editor)?;
        if !["undo","redo","save","save-close"].contains(&action.as_str()) { session.history.record(before_command,&session.snapshot()); }
        if !issues.is_empty() && message.is_empty() { message = "Please resolve the validation errors before saving.".into(); message_kind = "error"; }
        let html = views::edit_with_kind(&state.editor, &session, &actor, &state.fields, &issues, &message, message_kind)?;
        state.sessions.lock().unwrap().update(&actor, session)?;
        // Undo can restore the same HTML attributes a locally changed control already
        // has. Replacement resets dirty browser properties as well as markup; ordinary
        // commands and live validation retain morphing and Monaco's typing history.
        if ["undo", "redo"].contains(&action.as_str()) && fragment(&headers) {
            return Ok(stream_mode(vec![html], "replace"));
        }
        page(&state, &actor, &headers, html)
    }).await
}

fn related_session(
    state: &AppState,
    actor: &Actor,
    parent: &crate::sessions::EditSession,
    field: &str,
    form: &HashMap<String, String>,
) -> Result<crate::sessions::EditSession> {
    let editor = &state.editor;
    let picker = parent
        .picker
        .as_ref()
        .filter(|p| p.field == field)
        .ok_or_else(|| WebError::bad("Open the picker first"))?;
    let config = parent
        .draft
        .field(field)
        .filter(|f| !f.hidden && !f.read_only)
        .and_then(|f| f.schema.pointer("/x-relation/create"))
        .ok_or_else(|| WebError::bad("Related creation is not configured"))?;
    let value = parent.value().unwrap_or(serde_json::json!({}));
    let mut context = serde_json::json!({});
    let name = config["nameField"].as_str().unwrap_or("name");
    let query = form.get("picker_query").unwrap_or(&picker.query);
    if !query.is_empty() {
        context[name] = serde_json::json!(query);
        if let Some(copy) = config["copyTo"].as_str() {
            context[copy] = context[name].clone();
        }
    }
    if let (Some(target), Some(source)) = (
        config["parentField"].as_str(),
        config["sourceField"].as_str(),
    ) {
        if let Some(value) = value.get(source) {
            context[target] = value.clone();
        }
    }
    let mut child =
        crate::sessions::Sessions::default().start(editor, actor, &picker.collection, None)?;
    child.draft = editor.draft_for_id(
        &picker.collection,
        "",
        &editor.initial_value(&picker.collection, &context)?,
    )?;
    child.return_to = Some((parent.id.clone(), field.into()));
    child.mark_clean();
    state
        .sessions
        .lock()
        .unwrap()
        .register(actor, child.clone())?;
    Ok(child)
}

async fn export(
    State(state): App,
    Path((name, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response> {
    state.auth.actor(&headers).await?;
    blocking(move || {
        state.editor.require_operation(&name, "read")?;
        let d = state.editor.get(&name, &id)?;
        Ok((
            [
                (header::CONTENT_TYPE, "application/json".to_owned()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename*=UTF-8''{}.json", views::encode(&id)),
                ),
            ],
            serde_json::to_string_pretty(&d.value).map_err(WebError::internal)?,
        )
            .into_response())
    })
    .await
}
