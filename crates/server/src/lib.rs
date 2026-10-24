mod accounts;
pub mod assets;
pub mod auth;
mod deletion;
mod documents;
mod draft_lifecycle;
pub mod error;
pub mod fields;
mod media;
mod pickers;
pub mod projects;
pub mod request_logging;
mod routes;
pub mod sessions;
pub mod templates;
mod tree;
pub mod views;
use crate::{
    assets::Assets,
    auth::Auth,
    error::{Result, WebError},
    fields::Registry,
    sessions::Sessions,
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use structured_editor_core::Editor;

pub struct AppState {
    pub editor: Editor,
    pub project_id: String,
    pub projects: Arc<Vec<structured_editor_core::projects::Project>>,
    pub auth: Auth,
    pub assets: Assets,
    pub sessions: Arc<Mutex<Sessions>>,
    pub fields: Registry,
}
impl AppState {
    pub async fn open(
        project: &Path,
        state: &Path,
        assets: Assets,
        insecure: bool,
    ) -> Result<Arc<Self>> {
        let root = project.canonicalize().map_err(WebError::internal)?;
        std::fs::create_dir_all(state).map_err(WebError::internal)?;
        let state = state.canonicalize().map_err(WebError::internal)?;
        if state.starts_with(&root) || root.starts_with(&state) {
            return Err(WebError::bad(
                "Administrative storage and project content must be separate.",
            ));
        }
        let editor = tokio::task::spawn_blocking(move || Editor::open(root))
            .await
            .map_err(WebError::internal)??;
        let auth = Auth::open(&state.join("server.sqlite"), insecure).await?;
        Ok(Self::for_project(
            editor,
            auth,
            assets,
            "default".into(),
            Arc::new(vec![]),
        ))
    }
    pub(crate) fn for_project(
        editor: Editor,
        auth: Auth,
        assets: Assets,
        project_id: String,
        projects: Arc<Vec<structured_editor_core::projects::Project>>,
    ) -> Arc<Self> {
        let state = Arc::new(Self {
            editor,
            project_id,
            projects,
            auth,
            assets,
            sessions: Arc::new(Mutex::new(Sessions::default())),
            fields: Registry::default(),
        });
        let weak = Arc::downgrade(&state.sessions);
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                timer.tick().await;
                let Some(sessions) = weak.upgrade() else {
                    break;
                };
                sessions.lock().unwrap().expire();
            }
        });
        state
    }
}
pub fn router(state: Arc<AppState>) -> axum::Router {
    routes::router(state)
}
pub async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(WebError::internal)?
}
#[cfg(test)]
mod tests;

mod hierarchy;
mod preferences;
