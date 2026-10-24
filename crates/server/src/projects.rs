//! HTTP project selection; each project owns an Editor and its own draft sessions.
use crate::{
    AppState,
    assets::Assets,
    error::{Result, WebError},
    routes, views,
};
use axum::{
    Router,
    extract::{Form, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path, sync::Arc};
use structured_editor_core::{Editor, projects::Projects};
use tower::ServiceExt;

pub struct ProjectServer {
    default: String,
    states: BTreeMap<String, Arc<AppState>>,
    routers: BTreeMap<String, Router>,
}
impl ProjectServer {
    pub async fn open(
        catalog: Projects,
        storage: &Path,
        assets: Assets,
        insecure: bool,
    ) -> Result<Self> {
        let registrations = Arc::new(catalog.projects);
        let first = registrations
            .iter()
            .find(|p| p.id == catalog.default)
            .ok_or_else(|| WebError::bad("Missing default project"))?;
        let initial = AppState::open(&first.path, storage, assets.clone(), insecure).await?;
        let storage = storage.canonicalize().map_err(WebError::internal)?;
        let mut states = BTreeMap::new();
        for project in registrations.iter() {
            if storage.starts_with(&project.path) || project.path.starts_with(&storage) {
                return Err(WebError::bad(
                    "Administrative storage and project content must be separate.",
                ));
            }
            let editor = if project.id == catalog.default {
                initial.editor.clone()
            } else {
                let root = project.path.clone();
                crate::blocking(move || Ok(Editor::open(root)?)).await?
            };
            let state = AppState::for_project(
                editor,
                initial.auth.scoped(&project.id),
                assets.clone(),
                project.id.clone(),
                registrations.clone(),
            );
            states.insert(project.id.clone(), state);
        }
        let routers = states
            .iter()
            .map(|(id, state)| (id.clone(), routes::router(state.clone())))
            .collect();
        Ok(Self {
            default: catalog.default,
            states,
            routers,
        })
    }
    pub fn default_state(&self) -> Arc<AppState> {
        self.states[&self.default].clone()
    }
    pub fn router(self) -> Router {
        Router::new().fallback(dispatch).with_state(Arc::new(self))
    }
}
fn selected(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|s| s.strip_prefix("objekte_project="))
}
async fn dispatch(State(server): State<Arc<ProjectServer>>, request: Request) -> Response {
    let id = selected(request.headers())
        .filter(|id| server.routers.contains_key(*id))
        .unwrap_or(&server.default);
    // An older tab must not silently load or submit data from a different project.
    if let Some(expected) = request.headers().get("x-objekte-project") {
        if expected.to_str().ok() != Some(id) {
            let message = "The project changed in another tab. Reload this page before continuing.";
            return if routes::fragment(request.headers()) {
                routes::stream(vec![views::notice(message)])
            } else {
                WebError(StatusCode::CONFLICT, message.into()).into_response()
            };
        }
    }
    server.routers[id].clone().oneshot(request).await.unwrap()
}
#[derive(Deserialize)]
pub(crate) struct Selection {
    project: String,
    csrf: String,
}
pub(crate) async fn select(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<Selection>,
) -> Result<Response> {
    let actor = state.auth.actor(&headers).await?;
    state.auth.mutation(&actor, &form.csrf, false)?;
    if !state.projects.iter().any(|p| p.id == form.project) {
        return Err(WebError::bad("Unknown project"));
    }
    let mut response = Redirect::to("/").into_response();
    let cookie = format!(
        "objekte_project={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000{}",
        form.project,
        if state.auth.insecure_cookie {
            ""
        } else {
            "; Secure"
        }
    );
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie.parse().unwrap());
    Ok(response)
}
pub(crate) fn chooser(state: &AppState, actor: &crate::auth::Actor) -> String {
    let options = if state.projects.is_empty() {
        format!(
            "<option value=\"default\">{}</option>",
            views::escape(
                &state
                    .editor
                    .config()
                    .root
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            )
        )
    } else {
        state
            .projects
            .iter()
            .map(|p| {
                format!(
                    "<option value=\"{}\"{}>{}</option>",
                    views::escape(&p.id),
                    if p.id == state.project_id {
                        " selected"
                    } else {
                        ""
                    },
                    views::escape(&p.name)
                )
            })
            .collect()
    };
    format!(
        "<form class=\"project-chooser\" action=\"/projects/select\" method=\"post\" data-project-chooser data-project-id=\"{}\"><label for=\"project-select\" class=\"eyebrow\">Project</label><input type=\"hidden\" name=\"csrf\" value=\"{}\"><select id=\"project-select\" name=\"project\" aria-label=\"Project\"{}>{options}</select><noscript><button type=\"submit\">Open project</button></noscript></form>",
        views::escape(&state.project_id),
        views::escape(&actor.csrf),
        if state.projects.len() < 2 {
            " disabled"
        } else {
            ""
        }
    )
}
