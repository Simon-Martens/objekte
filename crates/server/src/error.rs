use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
#[derive(Debug)]
pub struct WebError(pub StatusCode, pub String);
pub type Result<T> = std::result::Result<T, WebError>;
impl WebError {
    pub fn bad(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(%error, "server error");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The operation could not be completed.".into(),
        )
    }
}
impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let mut response = (
            self.0,
            Html(format!(
                "<div id=\"notification-region\" role=\"alert\">{}</div>",
                crate::views::escape(&self.1)
            )),
        )
            .into_response();
        // The router turns typed errors into a DOM patch for Datastar requests.
        // Datastar ignores response bodies with a non-200 status.
        response.extensions_mut().insert(ErrorNotice(self.1));
        response
    }
}
#[derive(Clone)]
pub(crate) struct ErrorNotice(pub String);
impl From<structured_editor_core::Error> for WebError {
    fn from(value: structured_editor_core::Error) -> Self {
        use structured_editor_core::Error::*;
        let status = match &value {
            NotFound(_) => StatusCode::NOT_FOUND,
            Conflict(_) => StatusCode::CONFLICT,
            Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            InvalidPath(_) | Data(_) | Config(_) => StatusCode::BAD_REQUEST,
            Io(_) => return Self::internal(value),
        };
        Self(status, value.to_string())
    }
}
impl From<turso::Error> for WebError {
    fn from(e: turso::Error) -> Self {
        Self::internal(e)
    }
}
impl From<askama::Error> for WebError {
    fn from(e: askama::Error) -> Self {
        Self::internal(e)
    }
}
