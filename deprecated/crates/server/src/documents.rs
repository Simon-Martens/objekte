//! Editor settings and arbitrary schema-assigned JSON documents share one screen.
use crate::templates::Template;
use crate::{
    AppState,
    auth::Actor,
    blocking,
    error::{Result, WebError},
    routes, views,
};
use axum::{
    Router,
    extract::{Form, Path, Query, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use std::{collections::HashMap, sync::Arc};
type App = State<Arc<AppState>>;
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/documents", get(index))
        .route("/documents/{name}", get(open).post(save))
        .route("/documents/{name}/edit-sessions", post(start_document))
        .route("/documents/{name}/edit", get(edit_entry))
        .route(
            "/documents/{name}/validation",
            axum::routing::post(validate),
        )
        .route(
            "/settings/validation",
            axum::routing::post(validate_settings),
        )
        .route("/settings", get(settings).post(save_settings))
}
crate::templates::template!(DocumentForm<'_>, "document.html");
#[derive(serde::Serialize)]
struct DocumentForm<'a> {
    url: &'a str,
    validation_url: &'a str,
    raw: &'a str,
    revision: &'a str,
    file: &'a str,
    message: &'a str,
    csrf: &'a str,
    editable: bool,
}
fn render(
    a: &Actor,
    url: &str,
    title: &str,
    raw: &str,
    revision: &str,
    file: &str,
    message: &str,
) -> Result<String> {
    let validation_url = format!("{}/validation", url.split('?').next().unwrap_or(url));
    let body = DocumentForm {
        url,
        validation_url: &validation_url,
        raw,
        revision,
        file,
        message,
        csrf: &a.csrf,
        editable: a.editable(),
    }
    .render()?;
    views::panel(url, title, &body)
}
async fn index(State(s): App, h: HeaderMap) -> Result<Response> {
    let a = match routes::page_actor(&s, &h).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    let mut body = String::from("<section class=\"surface document-list\">");
    let mut count = 0;
    for (name, mapping) in &s.editor.config().documents {
        for file in &mapping.files {
            count += 1;
            body += &format!(
                "<div class=\"document-list-row\"><a class=\"document-row\" data-nav href=\"/documents/{}?file={}\"><i class=\"ri-braces-line\" aria-hidden=\"true\"></i><strong>{}</strong><span class=\"mono\">{}</span><i class=\"ri-arrow-right-s-line\" aria-hidden=\"true\"></i></a>",
                views::encode(name),
                views::encode(file),
                views::escape(
                    s.editor.document_schema(name)?["title"]
                        .as_str()
                        .unwrap_or(name)
                ),
                views::escape(file)
            );
            if a.editable() {
                body += &format!(
                    "<a class=\"icon-button\" data-nav href=\"/documents/{}/edit?file={}\" aria-label=\"Edit {}\"><i class=\"ri-edit-line\" aria-hidden=\"true\"></i></a>",
                    views::encode(name),
                    views::encode(file),
                    views::escape(file)
                );
            }
            body += "</div>";
        }
    }
    if count == 0 {
        body += "<div class=\"empty-state\"><h2>No documents configured</h2><p>Schema-assigned JSON files appear here.</p></div>";
    }
    body += "</section>";
    routes::page(
        &s,
        &a,
        &h,
        views::panel("/documents", "JSON documents", &body)?,
    )
}
async fn open(
    State(s): App,
    Path(name): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    h: HeaderMap,
) -> Result<Response> {
    let a = match routes::page_actor(&s, &h).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    blocking(move || {
        let file = q.get("file").cloned().unwrap_or_else(|| {
            s.editor
                .document_files(&name)
                .ok()
                .and_then(|v| v.first())
                .cloned()
                .unwrap_or_default()
        });
        let d = s.editor.json_document(&name, &file)?;
        let url = format!(
            "/documents/{}?file={}",
            views::encode(&name),
            views::encode(&file)
        );
        let schema = s.editor.document_editor_schema(&name)?;
        let draft = structured_editor_core::draft::Draft::document(&schema, &d.value);
        let content = crate::tree::preview_with_options(&s.editor, &draft, &d.value, &url, q.get("node").map(String::as_str).unwrap_or(""), &q, false);
        let node = q.get("node").map(String::as_str).unwrap_or("");
        let actions = if a.editable() {
            format!("<a class=\"button primary\" data-nav href=\"/documents/{}/edit?file={}&amp;node={}\"><i class=\"ri-edit-line\" aria-hidden=\"true\"></i>Edit</a>",views::encode(&name),views::encode(&file),views::encode(node))
        } else { String::new() };
        routes::page(&s, &a, &h, views::panel_with_actions(&format!("{url}&node={}", views::encode(q.get("node").map(String::as_str).unwrap_or(""))), &file, &content, &actions)?)
    })
    .await
}
async fn edit_entry(
    State(s): App,
    Path(name): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    h: HeaderMap,
) -> Result<Response> {
    let a = match routes::page_actor(&s, &h).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    if !a.editable() {
        return Err(WebError(
            axum::http::StatusCode::FORBIDDEN,
            "Editing permission required".into(),
        ));
    }
    blocking(move || {
        let file = q
            .get("file")
            .ok_or_else(|| WebError::bad("Document file required"))?;
        let node = q.get("node").map(String::as_str).unwrap_or("");
        let mut url = format!(
            "/documents/{}/edit?file={}",
            views::encode(&name),
            views::encode(file)
        );
        if !node.is_empty() {
            url += &format!("&node={}", views::encode(node));
        }
        let html = crate::draft_lifecycle::entry(
            &a,
            &url,
            &format!("/documents/{}/edit-sessions", views::encode(&name)),
            Some(file),
            node,
            false,
        )?;
        routes::page(&s, &a, &h, html)
    })
    .await
}
async fn start_document(
    State(s): App,
    Path(name): Path<String>,
    h: HeaderMap,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Response> {
    let a = routes::mutation_actor(&s, &h, &f).await?;
    blocking(move || {
        let file = f
            .get("file")
            .ok_or_else(|| WebError::bad("Document file required"))?;
        let key = format!(
            "/documents/{}/edit?file={}",
            views::encode(&name),
            views::encode(file)
        );
        match crate::draft_lifecycle::resume(&s, &a, &f, &key) {
            Ok(Some(session)) => {
                return routes::page(
                    &s,
                    &a,
                    &h,
                    views::edit(&s.editor, &session, &a, &s.fields, &[], "")?,
                );
            }
            Err(error) if error.0 == axum::http::StatusCode::NOT_FOUND => {
                let html = crate::draft_lifecycle::entry(
                    &a,
                    &key,
                    &format!("/documents/{}/edit-sessions", views::encode(&name)),
                    Some(file),
                    f.get("node").map(String::as_str).unwrap_or(""),
                    true,
                )?;
                return routes::page(&s, &a, &h, html);
            }
            Err(error) => return Err(error),
            Ok(None) => {}
        }
        let mut session =
            crate::sessions::Sessions::default().start_document(&s.editor, &a, &name, file)?;
        s.sessions.lock().unwrap().register(&a, session.clone())?;
        if let Some(path) = f.get("node") {
            if session
                .draft
                .at_path(path)
                .is_some_and(crate::tree::navigable)
            {
                session.view_path = path.clone();
                s.sessions.lock().unwrap().insert(session.clone());
            }
        }
        routes::page(
            &s,
            &a,
            &h,
            views::edit(&s.editor, &session, &a, &s.fields, &[], "")?,
        )
    })
    .await
}

async fn settings(State(s): App, h: HeaderMap) -> Result<Response> {
    let a = match routes::page_actor(&s, &h).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    blocking(move || {
        let d = s.editor.settings()?;
        routes::page(
            &s,
            &a,
            &h,
            crate::preferences::render(
                &s.editor,
                &a,
                &serde_json::to_string_pretty(&d.value).unwrap(),
                &d.revision,
                "",
                "success",
            )?
            .unwrap_or(render(
                &a,
                "/settings",
                "Editor settings",
                &serde_json::to_string_pretty(&d.value).unwrap(),
                &d.revision,
                "",
                "",
            )?),
        )
    })
    .await
}
async fn save(
    State(s): App,
    Path(name): Path<String>,
    h: HeaderMap,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Response> {
    submit(s, Some(name), h, f).await
}
async fn save_settings(
    State(s): App,
    h: HeaderMap,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Response> {
    submit(s, None, h, f).await
}
async fn submit(
    s: Arc<AppState>,
    name: Option<String>,
    h: HeaderMap,
    f: HashMap<String, String>,
) -> Result<Response> {
    let a = routes::mutation_actor(&s, &h, &f).await?;
    blocking(move || {
        let raw = if name.is_none() && f.contains_key("settings_form") {
            match crate::preferences::value(&s.editor, &f) {
                Ok(v) => serde_json::to_string_pretty(&v).unwrap(),
                Err(e) => {
                    let mut submitted = s.editor.settings()?.value;
                    if let Some(props) = s.editor.settings_schema()?["properties"].as_object() {
                        for (key, schema) in props {
                            if let Some(raw) = f.get(&format!("setting.{key}")) {
                                submitted[key] =
                                    if schema["type"] == "string" && schema.get("enum").is_none() {
                                        serde_json::json!(raw)
                                    } else {
                                        serde_json::from_str(raw)
                                            .unwrap_or_else(|_| serde_json::json!(raw))
                                    };
                            } else if schema["type"] == "boolean" {
                                submitted[key] = serde_json::json!(false);
                            }
                        }
                    }
                    return routes::page(
                        &s,
                        &a,
                        &h,
                        crate::preferences::render(
                            &s.editor,
                            &a,
                            &serde_json::to_string(&submitted).unwrap(),
                            f.get("revision").map(String::as_str).unwrap_or(""),
                            &e.1,
                            "error",
                        )?
                        .unwrap(),
                    );
                }
            }
        } else {
            f.get("raw").cloned().unwrap_or_default()
        };
        let mut revision = f
            .get("revision")
            .cloned()
            .ok_or_else(|| WebError::bad("Revision required"))?;
        let file = f.get("file").cloned().unwrap_or_default();
        let url = name
            .as_ref()
            .map(|n| {
                format!(
                    "/documents/{}?file={}",
                    views::encode(n),
                    views::encode(&file)
                )
            })
            .unwrap_or("/settings".into());
        let result = (|| -> Result<String> {
            let v = serde_json::from_str(&raw).map_err(|e| WebError::bad(e.to_string()))?;
            if f.get("action").is_some_and(|s| s == "validate") {
                let issues = if let Some(n) = &name {
                    s.editor.validate_document(n, &v)?
                } else {
                    s.editor.validate_settings(&v)?
                };
                if !issues.is_empty() {
                    return Err(WebError::bad(
                        issues
                            .iter()
                            .map(|i| format!("{}: {}", i.path, i.message))
                            .collect::<Vec<_>>()
                            .join("; "),
                    ));
                }
                return Ok("The document is valid.".into());
            }
            let saved = if let Some(n) = &name {
                s.editor.save_json_document(n, &file, v, &revision)
            } else {
                s.editor.save_settings(v, &revision)
            };
            match saved {
                Ok(d) => {
                    revision = d.revision;
                    Ok("Saved to disk.".into())
                }
                Err(structured_editor_core::Error::Validation(issues)) => Err(WebError::bad(
                    issues
                        .iter()
                        .map(|i| format!("{}: {}", i.path, i.message))
                        .collect::<Vec<_>>()
                        .join("; "),
                )),
                Err(e) => Err(e.into()),
            }
        })();
        let notice_kind = if result.is_ok() { "success" } else { "error" };
        let message = result.unwrap_or_else(|e| e.1);
        if name.is_none() {
            if let Some(html) =
                crate::preferences::render(&s.editor, &a, &raw, &revision, &message, notice_kind)?
            {
                return routes::page(
                    &s,
                    &a,
                    &h,
                    html.replace(
                        "data-notification=\"success\"",
                        &format!("data-notification=\"{notice_kind}\""),
                    ),
                );
            }
        }
        routes::page(
            &s,
            &a,
            &h,
            render(
                &a,
                &url,
                if name.is_some() {
                    &file
                } else {
                    "Editor settings"
                },
                &raw,
                &revision,
                &file,
                &message,
            )?
            .replace(
                "data-notification=\"success\"",
                &format!("data-notification=\"{notice_kind}\""),
            ),
        )
    })
    .await
}

async fn validate(
    State(s): App,
    Path(name): Path<String>,
    h: HeaderMap,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Response> {
    validation(s, Some(name), h, f).await
}
async fn validate_settings(
    State(s): App,
    h: HeaderMap,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Response> {
    validation(s, None, h, f).await
}
async fn validation(
    s: Arc<AppState>,
    name: Option<String>,
    h: HeaderMap,
    f: HashMap<String, String>,
) -> Result<Response> {
    routes::mutation_actor(&s, &h, &f).await?;
    blocking(move || {
        let result = (|| -> Result<Vec<structured_editor_core::ValidationIssue>> {
            let value = if name.is_none(){crate::preferences::value(&s.editor,&f)?}else{serde_json::from_str(f.get("raw").map(String::as_str).unwrap_or("")).map_err(|e| WebError::bad(e.to_string()))?};
            Ok(if let Some(name) = &name { s.editor.validate_document(name, &value)? }
                else { s.editor.validate_settings(&value)? })
        })();
        let (valid, message) = match result {
            Ok(issues) if issues.is_empty() => (true, "Valid JSON · all schema checks passed".to_owned()),
            Ok(issues) => (false, issues.iter().map(|i| format!("{}: {}", i.path, i.message)).collect::<Vec<_>>().join("; ")),
            Err(error) => (false, error.1),
        };
        Ok(routes::stream(vec![format!("<div id=\"document-validation\" role=\"status\" aria-live=\"polite\" class=\"{}\">{}</div>",
            if valid { "help valid-text" } else { "help invalid-text" }, views::escape(&message))]))
    }).await
}
