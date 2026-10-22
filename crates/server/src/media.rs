//! Media collection HTTP adapter. Filesystem rules and reference checks stay in core.
use crate::{
    AppState,
    auth::Actor,
    blocking,
    error::{Result, WebError},
    routes, views,
};
use axum::{
    Router,
    extract::{DefaultBodyLimit, Form, Multipart, Path, Query, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc};
use structured_editor_core::MediaKind;
type App = State<Arc<AppState>>;
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        // Keep imported Markdown links working without rewriting source content.
        .route("/api/media/file", get(legacy_file))
        .route("/media/{name}", get(browse))
        .route("/media/{name}/file", get(file))
        .route("/media/{name}/replace", get(replace_page))
        .route("/media/{name}/references", get(references))
        .route("/media/{name}/actions/{action}", post(action))
        .route(
            "/media/{name}/upload",
            post(upload).layer(DefaultBodyLimit::max(52 * 1024 * 1024)),
        )
}
pub fn render(
    state: &AppState,
    actor: &Actor,
    name: &str,
    path: &str,
    q: &str,
    offset: usize,
    message: &str,
) -> Result<String> {
    let editor = state.editor.for_media(name)?;
    let config = &state.editor.config().media_collections[name];
    let entries = if q.is_empty() {
        editor.media_list(path, offset, 51)?
    } else {
        editor.media_search(q, offset, 51)?
    };
    let base = format!("/media/{}", views::encode(name));
    let url = format!(
        "{base}?path={}&q={}&offset={offset}",
        views::encode(path),
        views::encode(q)
    );
    let mut body = String::new();
    if !message.is_empty() {
        body += &format!(
            "<p class=\"notice\" role=\"status\">{}</p>",
            views::escape(message)
        );
    }
    body += &format!(
        "<div class=\"media-breadcrumb\"><a data-nav href=\"{base}\"><i class=\"ri-folder-line\" aria-hidden=\"true\"></i> Collection root</a>"
    );
    if !path.is_empty() {
        let mut current = String::new();
        for part in path.split('/') {
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(part);
            body += &format!(
                "<i class=\"ri-arrow-right-s-line\" aria-hidden=\"true\"></i><a data-nav href=\"{base}?path={}\">{}</a>",
                views::encode(&current),
                views::escape(part)
            );
        }
    }
    body += "</div><section class=\"surface media-list\" aria-label=\"Media files\"><div class=\"list-toolbar\">";
    body += &format!(
        "<form class=\"search-form\" action=\"{base}\" data-indicator:_pending data-on:submit__prevent=\"@get(el.getAttribute('action')+'?'+new URLSearchParams(new FormData(el)),{{filterSignals:{{include:/^$/}}}})\"><input type=\"hidden\" name=\"path\" value=\"{}\"><label class=\"search-input\"><i class=\"ri-search-line\" aria-hidden=\"true\"></i><span class=\"sr-only\">Search files</span><input name=\"q\" type=\"search\" value=\"{}\" placeholder=\"Search this collection…\"></label><button>Search</button></form><div class=\"actions\">",
        views::escape(path),
        views::escape(q)
    );
    if actor.editable() {
        if editor.allows(name, "mkdir") {
            body += &format!(
                "<details class=\"action-menu\"><summary class=\"button\"><i class=\"ri-folder-add-line\" aria-hidden=\"true\"></i>New folder</summary><div class=\"menu-panel\"><form method=\"post\" action=\"{base}/actions/mkdir\" data-indicator:_pending data-on:submit__prevent=\"@post(el.getAttribute('action'),{{contentType:'form',retry:'never'}})\">{}<label>Folder name<input name=\"name\" required placeholder=\"folder-name\"></label><button class=\"primary\">Create folder</button></form></div></details>",
                hidden(actor, path)
            );
        }
        if editor.allows(name, "upload") {
            body += &format!(
                "<details class=\"action-menu\"><summary class=\"button primary\"><i class=\"ri-upload-2-line\" aria-hidden=\"true\"></i>Upload file</summary><div class=\"menu-panel\"><form method=\"post\" enctype=\"multipart/form-data\" action=\"{base}/upload\" data-indicator:_pending data-on:submit__prevent=\"@post(el.getAttribute('action'),{{contentType:'form',retry:'never'}})\">{}<label>Choose file<input type=\"file\" name=\"file\" required></label><p class=\"help\">Up to 50 MiB. Existing files are preserved.</p><button class=\"primary\"><i class=\"ri-upload-2-line\" aria-hidden=\"true\"></i>Upload</button></form></div></details>",
                hidden(actor, path)
            );
        }
    }
    body += "</div></div><div class=\"record-columns media-columns\" aria-hidden=\"true\"><span>Name</span><span>References</span><span>Size</span><span></span></div>";
    for entry in entries.iter().take(50) {
        let p = views::encode(&entry.path);
        let folder = matches!(entry.kind, MediaKind::Directory);
        let extension = entry
            .name
            .rsplit_once('.')
            .map(|(_, x)| x.to_ascii_lowercase())
            .unwrap_or_default();
        let icon = if folder {
            "ri-folder-3-line"
        } else {
            match extension.as_str() {
                "png" | "jpg" | "jpeg" | "webp" | "tif" | "tiff" | "gif" | "svg" => "ri-image-line",
                "pdf" => "ri-file-pdf-2-line",
                "mp3" | "wav" | "ogg" => "ri-file-music-line",
                "mp4" | "webm" => "ri-film-line",
                _ => "ri-file-line",
            }
        };
        let size = if folder {
            "—".into()
        } else if entry.size >= 1024 * 1024 {
            format!("{:.1} MiB", entry.size as f64 / 1048576.0)
        } else if entry.size >= 1024 {
            format!("{:.1} KiB", entry.size as f64 / 1024.0)
        } else {
            format!("{} B", entry.size)
        };
        let thumbnail = !folder
            && matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif")
            && state
                .editor
                .settings()
                .ok()
                .is_none_or(|d| d.value["mediaThumbnails"] != false);
        let visual = if thumbnail {
            format!(
                "<img class=\"media-thumbnail\" src=\"{base}/file?path={p}\" alt=\"\" loading=\"lazy\">"
            )
        } else {
            format!("<i class=\"{icon}\" aria-hidden=\"true\"></i>")
        };
        body += &format!(
            "<section class=\"media-row\"><div class=\"media-name\">{visual}<div><a {} href=\"{}\">{}</a><small>{}</small></div></div><span class=\"media-size mono\">{size}</span>",
            if folder { "data-nav" } else { "" },
            if folder {
                format!("{base}?path={p}")
            } else {
                format!("{base}/file?path={p}")
            },
            views::escape(&entry.name),
            if folder {
                "FOLDER".into()
            } else {
                views::escape(&entry.path)
            }
        );
        if actor.editable()
            && (editor.allows(name, "move")
                || editor.allows(name, "delete")
                || editor.allows(name, "replace")
                || editor.allows(name, "copy"))
        {
            body += &format!(
                "<details class=\"action-menu\"><summary class=\"icon-button\" aria-label=\"Actions for {}\" title=\"File actions\"><i class=\"ri-more-2-line\" aria-hidden=\"true\"></i></summary><div class=\"menu-panel\">",
                views::escape(&entry.name)
            );
            if !folder {
                body += &format!(
                    "<a class=\"menu-action\" href=\"{base}/file?path={p}&download=1\"><i class=\"ri-download-line\" aria-hidden=\"true\"></i>Download</a><button type=\"button\" class=\"menu-action\" data-copy=\"{}\"><i class=\"ri-file-copy-line\" aria-hidden=\"true\"></i>Copy path</button>",
                    views::escape(&entry.path)
                );
                if editor.allows(name, "replace") {
                    body += &format!(
                        "<a class=\"menu-action\" data-nav href=\"{base}/replace?path={p}\"><i class=\"ri-upload-2-line\" aria-hidden=\"true\"></i>Replace file…</a>"
                    );
                }
            }
            for (action, label, icon) in [
                ("move", "Move or rename", "ri-file-transfer-line"),
                ("copy", "Duplicate file", "ri-file-copy-line"),
                ("delete", "Delete", "ri-delete-bin-line"),
            ] {
                if editor.allows(name, action) && (action != "copy" || !folder) {
                    body += &format!(
                        "<form method=\"post\" action=\"{base}/actions/{action}\" data-indicator:_pending data-on:submit__prevent=\"{}@post(el.getAttribute('action'),{{contentType:'form',retry:'never'}})\">{}<input type=\"hidden\" name=\"source\" value=\"{}\">{}<button class=\"{}\"><i class=\"{icon}\" aria-hidden=\"true\"></i>{label}</button></form>",
                        if action == "delete" {
                            "if(confirm('Delete this file or empty folder?')) "
                        } else {
                            ""
                        },
                        hidden(actor, path),
                        views::escape(&entry.path),
                        if action == "move" || action == "copy" {
                            format!(
                                "<label>Destination path<input class=\"mono\" name=\"target\" value=\"{}\" required></label>",
                                views::escape(&entry.path)
                            )
                        } else {
                            String::new()
                        },
                        if action == "delete" { "danger" } else { "" }
                    );
                }
            }
            body += "</div></details>";
        } else {
            body += "<span></span>";
        }
        if !folder {
            let target = reference_id(&entry.path);
            body += &format!(
                "<details class=\"media-references\" data-on:toggle=\"if(el.open) @get('{base}/references?path={p}',{{filterSignals:{{include:/^$/}},retry:'never'}})\"><summary><i class=\"ri-links-line\" aria-hidden=\"true\"></i>References</summary><div id=\"{target}\"><p class=\"help\">Loading references…</p></div></details>"
            );
        }
        body += "</section>";
    }
    if entries.is_empty() {
        body += "<div class=\"empty-state\"><i class=\"ri-folder-open-line\" aria-hidden=\"true\"></i><h2>No files found</h2><p>Upload a file, create a folder or try another search.</p></div>";
    }
    body += &format!(
        "<footer class=\"list-footer\"><span class=\"list-count mono\">{} items shown</span><div class=\"actions\">",
        entries.len().min(50)
    );
    if offset > 0 {
        body += &format!(
            "<a class=\"button\" data-nav href=\"{base}?path={}&q={}&offset={}\">Previous</a>",
            views::encode(path),
            views::encode(q),
            offset.saturating_sub(50)
        );
    }
    if entries.len() > 50 {
        body += &format!(
            "<a class=\"button\" data-nav href=\"{base}?path={}&q={}&offset={}\">Next</a>",
            views::encode(path),
            views::encode(q),
            offset.saturating_add(50)
        );
    }
    body += "</div></footer></section>";
    views::panel(&url, &config.label, &body)
}
fn reference_id(path: &str) -> String {
    format!(
        "media-refs-{}",
        hex::encode(Sha256::digest(path.as_bytes()))
    )
}
async fn references(
    State(state): App,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response> {
    let actor = match routes::page_actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(response) => return Ok(response),
    };
    blocking(move || {
        state.editor.require_operation(&name, "read")?;
        let path = query.get("path").map(String::as_str).unwrap_or("");
        let references = state.editor.for_media(&name)?.media_references(path)?;
        let mut body = format!("<div id=\"{}\">", reference_id(path));
        if references.is_empty() {
            body += "<p>No explicit field references.</p>";
        }
        for reference in references {
            body += &format!(
                "<p><a data-nav href=\"{}\">{}/{}</a> <code>{}</code></p>",
                views::record_url(&reference.collection, &reference.record_id),
                views::escape(&reference.collection),
                views::escape(&reference.record_id),
                views::escape(&reference.path)
            );
        }
        body += "</div>";
        if routes::fragment(&headers) {
            Ok(routes::stream(vec![body]))
        } else {
            routes::page(
                &state,
                &actor,
                &headers,
                views::panel(
                    &format!("/media/{name}/references"),
                    "File references",
                    &body,
                )?,
            )
        }
    })
    .await
}
fn hidden(actor: &Actor, path: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"path\" value=\"{}\">",
        actor.csrf,
        views::escape(path)
    )
}
async fn browse(
    State(state): App,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response> {
    let actor = match routes::page_actor(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    blocking(move || {
        let html = render(
            &state,
            &actor,
            &name,
            query.get("path").map(String::as_str).unwrap_or(""),
            query.get("q").map(String::as_str).unwrap_or(""),
            query
                .get("offset")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            "",
        )?;
        routes::page(&state, &actor, &headers, html)
    })
    .await
}
async fn action(
    State(state): App,
    Path((name, action)): Path<(String, String)>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response> {
    let actor = routes::mutation_actor(&state, &headers, &form).await?;
    blocking(move || {
        let editor = state.editor.for_media(&name)?;
        let path = form.get("path").map(String::as_str).unwrap_or("");
        let source = form.get("source").map(String::as_str).unwrap_or("");
        match action.as_str() {
            "mkdir" => {
                let child = form
                    .get("name")
                    .ok_or_else(|| WebError::bad("Folder name required"))?;
                structured_editor_core::safe_name(child)?;
                editor.media_mkdir(&if path.is_empty() {
                    child.clone()
                } else {
                    format!("{path}/{child}")
                })?;
            }
            "move" => {
                editor.media_move(source, form.get("target").map(String::as_str).unwrap_or(""))?
            }
            "delete" => editor.media_delete(source)?,
            "copy" => {
                editor.media_copy(source, form.get("target").map(String::as_str).unwrap_or(""))?
            }
            _ => return Err(WebError::bad("Unknown media operation")),
        };
        routes::page(
            &state,
            &actor,
            &headers,
            render(&state, &actor, &name, path, "", 0, "Media updated.")?,
        )
    })
    .await
}
async fn upload(
    State(state): App,
    Path(name): Path<String>,
    headers: HeaderMap,
    mut data: Multipart,
) -> Result<Response> {
    let actor = state.auth.actor(&headers).await?;
    let mut csrf = String::new();
    let mut path = String::new();
    let mut file = None;
    let mut replacement = String::new();
    let mut revision = String::new();
    while let Some(field) = data.next_field().await.map_err(WebError::internal)? {
        match field.name().unwrap_or("") {
            "csrf" => csrf = field.text().await.map_err(WebError::internal)?,
            "path" => path = field.text().await.map_err(WebError::internal)?,
            "replacement" => replacement = field.text().await.map_err(WebError::internal)?,
            "revision" => revision = field.text().await.map_err(WebError::internal)?,
            "file" => {
                let name = field
                    .file_name()
                    .ok_or_else(|| WebError::bad("Filename required"))?
                    .to_owned();
                structured_editor_core::safe_name(&name)?;
                let bytes = field.bytes().await.map_err(WebError::internal)?;
                file = Some((name, bytes));
            }
            _ => {}
        }
    }
    state.auth.mutation(&actor, &csrf, true)?;
    let (filename, bytes) = file.ok_or_else(|| WebError::bad("Select a file"))?;
    blocking(move || {
        let target = if path.is_empty() {
            filename
        } else {
            format!("{path}/{filename}")
        };
        let media = state.editor.for_media(&name)?;
        if replacement.is_empty() {
            media.media_upload(&target, &bytes, 50 * 1024 * 1024)?;
        } else {
            let extension = |p: &str| {
                std::path::Path::new(p)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase()
            };
            if extension(&target) != extension(&replacement) {
                return Err(WebError::bad(
                    "Replacement must have the same file extension.",
                ));
            }
            media.media_replace(&replacement, &bytes, &revision, 50 * 1024 * 1024)?;
        }
        routes::page(
            &state,
            &actor,
            &headers,
            render(
                &state,
                &actor,
                &name,
                &path,
                "",
                0,
                if replacement.is_empty() {
                    "File uploaded."
                } else {
                    "File replaced. Existing references keep the same path."
                },
            )?,
        )
    })
    .await
}
async fn legacy_file(
    state: App,
    query: Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response> {
    file(state, Path("media".to_owned()), query, headers).await
}
async fn file(
    State(state): App,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response> {
    state.auth.actor(&headers).await?;
    let path = query
        .get("path")
        .cloned()
        .ok_or_else(|| WebError::bad("Path required"))?;
    let download = query.get("download").is_some_and(|v| v == "1");
    blocking(move || {
        let bytes = state.editor.for_media(&name)?.media_read(&path)?;
        let extension = std::path::Path::new(&path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        let kind = match extension.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            _ => "application/octet-stream",
        };
        let disposition = format!(
            "{}; filename*=UTF-8''{}",
            if kind.starts_with("image/") && !download {
                "inline"
            } else {
                "attachment"
            },
            views::encode(
                std::path::Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("download")
            )
        );
        Ok((
            [
                (header::CONTENT_TYPE, kind),
                (header::CONTENT_DISPOSITION, disposition.as_str()),
                (header::CONTENT_SECURITY_POLICY, "sandbox"),
            ],
            bytes,
        )
            .into_response())
    })
    .await
}

async fn replace_page(
    State(state): App,
    Path(name): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response> {
    let actor = match routes::page_actor(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    if !actor.editable() {
        return Err(WebError(
            axum::http::StatusCode::FORBIDDEN,
            "Read-only account".into(),
        ));
    }
    blocking(move||{
        let path=q.get("path").ok_or_else(||WebError::bad("Path required"))?;
        let media=state.editor.for_media(&name)?;media.require_operation(&name,"replace")?;
        let revision=media.media_revision(path)?;
        let parent=path.rsplit_once('/').map(|(p,_)|p).unwrap_or("");let base=format!("/media/{}",views::encode(&name));
        let body=format!("<section class=\"surface replace-panel\"><div class=\"section-heading\"><h2><i class=\"ri-file-upload-line\" aria-hidden=\"true\"></i>{}</h2></div><form method=\"post\" enctype=\"multipart/form-data\" action=\"{base}/upload\" data-on:submit__prevent=\"@post(el.getAttribute('action'),{{contentType:'form',retry:'never'}})\">{}<input type=\"hidden\" name=\"replacement\" value=\"{}\"><input type=\"hidden\" name=\"revision\" value=\"{revision}\"><p>The replacement keeps this filename and every existing reference. Select a file of the same format.</p><label>Replacement file<input type=\"file\" name=\"file\" required></label><div class=\"actions\"><a class=\"button\" data-nav href=\"{base}?path={}\">Cancel</a><button class=\"primary\"><i class=\"ri-upload-2-line\" aria-hidden=\"true\"></i>Replace file</button></div></form></section>",views::escape(path),hidden(&actor,parent),views::escape(path),views::encode(parent));
        routes::page(&state,&actor,&headers,views::panel(&format!("{base}/replace?path={}",views::encode(path)),"Replace media file",&body)?)
    }).await
}
