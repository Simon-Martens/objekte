//! Request-scoped interface language. Stored field values and draft schemas never change.
use crate::{
    AppState,
    error::{Result, WebError},
    views,
};
use axum::{
    extract::{Form, Request, State},
    http::{HeaderMap, header},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use structured_editor_core::Editor;

tokio::task_local! { static REQUEST_LANGUAGE: String; }
thread_local! { static RENDER_LANGUAGE: RefCell<Option<String>> = const { RefCell::new(None) }; }

pub fn language() -> String {
    RENDER_LANGUAGE
        .with(|value| value.borrow().clone())
        .or_else(|| REQUEST_LANGUAGE.try_with(Clone::clone).ok())
        .unwrap_or_else(|| "en".into())
}
pub fn with_language<T>(language: &str, work: impl FnOnce() -> T) -> T {
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            RENDER_LANGUAGE.with(|v| *v.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(RENDER_LANGUAGE.with(|v| v.replace(Some(language.into()))));
    work()
}
pub fn languages(editor: &Editor) -> Vec<(String, String)> {
    editor.config().ui["i18n"]["languages"]
        .as_object()
        .map(|options| {
            options
                .iter()
                .filter_map(|(code, label)| {
                    (valid_code(code))
                        .then(|| label.as_str().map(|label| (code.clone(), label.into())))
                        .flatten()
                })
                .collect()
        })
        .unwrap_or_default()
}
fn valid_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 35
        && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}
pub fn validate(editor: &Editor, code: &str) -> Result<()> {
    if languages(editor).iter().any(|(key, _)| key == code) {
        Ok(())
    } else {
        Err(WebError::bad("Unknown interface language"))
    }
}
fn selected(editor: &Editor, headers: &HeaderMap) -> String {
    let preferred = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .map(str::trim)
                .find_map(|v| v.strip_prefix("objekte_language="))
        });
    let options = languages(editor);
    preferred
        .filter(|code| options.iter().any(|(key, _)| key == code))
        .or_else(|| {
            editor.config().ui["i18n"]["defaultLanguage"]
                .as_str()
                .filter(|code| options.iter().any(|(key, _)| key == code))
        })
        .unwrap_or("en")
        .into()
}
pub async fn middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let code = selected(&state.editor, request.headers());
    REQUEST_LANGUAGE
        .scope(code.clone(), async move {
            let mut response = next.run(request).await;
            if !response.headers().contains_key(header::CONTENT_LANGUAGE) {
                response
                    .headers_mut()
                    .insert(header::CONTENT_LANGUAGE, code.parse().unwrap());
            }
            response
        })
        .await
}

/// UI files may attach translations to collection names, fields, descriptions and options.
pub fn translation(options: &Value) -> Option<&Value> {
    let code = language();
    let messages = options
        .get("translations")
        .or_else(|| options.get("x-ui-translations"))?;
    messages.get(&code).or_else(|| {
        code.split_once('-')
            .and_then(|(base, _)| messages.get(base))
    })
}
pub fn text<'a>(options: &'a Value, key: &str, fallback: &'a str) -> &'a str {
    let code = language();
    let messages = options
        .get("translations")
        .or_else(|| options.get("x-ui-translations"));
    messages
        .and_then(|v| {
            v.get(&code)
                .or_else(|| code.split_once('-').and_then(|(base, _)| v.get(base)))
        })
        .and_then(|v| v.get(key))
        .and_then(Value::as_str)
        .or_else(|| options.get(key).and_then(Value::as_str))
        .unwrap_or(fallback)
}
pub fn field<'a>(schema: &'a Value, fallback: &'a str) -> &'a str {
    text(schema, "title", fallback)
}
pub fn collection<'a>(editor: &'a Editor, name: &str) -> String {
    editor
        .config()
        .collections
        .get(name)
        .map(|c| text(&c.ui, "label", &c.label).to_owned())
        .unwrap_or_else(|| name.into())
}
pub fn t(key: &str) -> String {
    static GERMAN: OnceLock<HashMap<String, String>> = OnceLock::new();
    let code = language();
    if code == "de" || code.starts_with("de-") {
        GERMAN
            .get_or_init(|| serde_json::from_str(include_str!("../locales/de.json")).unwrap())
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.into())
    } else {
        key.into()
    }
}
pub fn message(key: &str, values: &[(&str, &str)]) -> String {
    let mut text = t(key);
    for (name, value) in values {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}
pub fn chooser(state: &AppState, actor: &crate::auth::Actor) -> String {
    let options = languages(&state.editor);
    if options.is_empty() {
        return "<span id=\"language-picker\" hidden></span>".into();
    }
    let mut html = format!(
        "<form id=\"language-picker\" action=\"/ui/language\" method=\"post\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"return_to\" value=\"/\"><label class=\"language-picker-label\"><i class=\"ri-translate-2\" aria-hidden=\"true\"></i><span class=\"sr-only\">{}</span><select name=\"language\" data-language-picker data-attr:disabled=\"$_pending\">",
        views::escape(&actor.csrf),
        views::escape(&t("Interface language"))
    );
    for (code, label) in options {
        html += &format!(
            "<option value=\"{}\" {}>{}</option>",
            views::escape(&code),
            if language() == code { "selected" } else { "" },
            views::escape(&label)
        );
    }
    html + "</select></label><noscript><button>OK</button></noscript></form>"
}
pub fn cookie(state: &AppState, response: &mut Response, code: &str) {
    response.headers_mut().append(
        header::SET_COOKIE,
        format!(
            "objekte_language={code}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000{}",
            if state.auth.insecure_cookie {
                ""
            } else {
                "; Secure"
            }
        )
        .parse()
        .unwrap(),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_LANGUAGE, code.parse().unwrap());
}
pub async fn select(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response> {
    let actor = state.auth.actor(&headers).await?;
    state.auth.mutation(
        &actor,
        form.get("csrf").map(String::as_str).unwrap_or(""),
        false,
    )?;
    let code = form.get("language").map(String::as_str).unwrap_or("");
    validate(&state.editor, code)?;
    let url = form
        .get("return_to")
        .filter(|s| s.starts_with('/') && !s.starts_with("//") && !s.contains(['\\', '\r', '\n']))
        .map(String::as_str)
        .unwrap_or("/");
    let mut response = Redirect::to(url).into_response();
    cookie(&state, &mut response, code);
    Ok(response)
}
