//! One template language in both modes: bundled for deployment, read from disk in dev.
use crate::error::{Result, WebError};
use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use serde::Serialize;
use std::{path::PathBuf, sync::OnceLock};

include!(concat!(env!("OUT_DIR"), "/templates.rs"));

enum Templates {
    Bundled(Environment<'static>),
    Directory(PathBuf),
}

fn environment() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::Html);
    // Match the escaping used by hand-built fragments. In particular, keep URL
    // slashes literal: navigation derives the active section from fragment URLs.
    env.set_formatter(|out, state, value| {
        if *state.auto_escape() == AutoEscape::Html && !value.is_safe() && !value.is_undefined() {
            write!(out, "{}", crate::views::escape(&value.to_string()))?;
            Ok(())
        } else {
            minijinja::escape_formatter(out, state, value)
        }
    });
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_syntax(
        minijinja::syntax::SyntaxConfig::builder()
            .keep_trailing_newline(true)
            .build()
            .unwrap(),
    );
    env
}

impl Templates {
    fn bundled() -> std::result::Result<Self, minijinja::Error> {
        let mut env = environment();
        for &(name, source) in BUNDLED {
            env.add_template(name, source)?;
        }
        Ok(Self::Bundled(env))
    }

    fn render(&self, name: &str, context: impl Serialize) -> Result<String> {
        match self {
            Self::Bundled(env) => Ok(env
                .get_template(name)?
                .render(minijinja::value::Serde(context))?),
            Self::Directory(path) => {
                // A fresh environment also reloads includes, new files and syntax fixes,
                // without accumulating old compiled templates over a long dev session.
                let mut env = environment();
                env.set_loader(minijinja::path_loader(path));
                env.get_template(name)
                    .and_then(|template| template.render(minijinja::value::Serde(context)))
                    .map_err(|error| {
                        tracing::error!(%error, "development template error");
                        WebError(
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Template error: {error:#}"),
                        )
                    })
            }
        }
    }
}

static TEMPLATES: OnceLock<std::result::Result<Templates, minijinja::Error>> = OnceLock::new();

/// Called once at startup. Tests and library users default to bundled templates.
pub fn init(dev: bool) -> Result<()> {
    let templates = TEMPLATES.get_or_init(|| {
        if dev {
            Ok(Templates::Directory(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates"),
            ))
        } else {
            Templates::bundled()
        }
    });
    templates.as_ref().map(|_| ()).map_err(WebError::internal)
}

pub(crate) trait Template: Serialize {
    const NAME: &'static str;
    fn render(&self) -> Result<String> {
        TEMPLATES
            .get_or_init(Templates::bundled)
            .as_ref()
            .map_err(WebError::internal)?
            .render(Self::NAME, self)
    }
}

macro_rules! template {
    ($context:ty, $name:literal) => {
        impl crate::templates::Template for $context {
            const NAME: &'static str = $name;
        }
    };
}
pub(crate) use template;

/// Expose only the account properties used by markup.
#[derive(Serialize)]
pub(crate) struct ActorContext<'a> {
    name: &'a str,
    role: &'a str,
    csrf: &'a str,
}
impl<'a> From<&'a crate::auth::Actor> for ActorContext<'a> {
    fn from(actor: &'a crate::auth::Actor) -> Self {
        Self {
            name: &actor.name,
            role: &actor.role,
            csrf: &actor.csrf,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn disk_templates_reload_includes_and_recover_from_errors() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();
        std::fs::write(
            path.join("page.html"),
            "<h1>{{ title }}</h1>{% include 'part.html' %}",
        )
        .unwrap();
        std::fs::write(path.join("part.html"), "before").unwrap();
        let templates = Templates::Directory(path.to_owned());
        let context = json!({"title": "<unsafe>"});
        let render = || templates.render("page.html", &context);
        assert_eq!(render().unwrap(), "<h1>&lt;unsafe&gt;</h1>before");
        std::fs::write(path.join("part.html"), "after").unwrap();
        assert_eq!(render().unwrap(), "<h1>&lt;unsafe&gt;</h1>after");
        std::fs::write(path.join("part.html"), "{% broken %}").unwrap();
        let error = render().unwrap_err();
        assert!(error.1.contains("part.html"));
        std::fs::write(path.join("part.html"), "{% include 'new.html' %}").unwrap();
        std::fs::write(path.join("new.html"), "recovered").unwrap();
        assert!(render().unwrap().ends_with("recovered"));
        std::fs::remove_file(path.join("new.html")).unwrap();
        assert!(render().is_err());
        // A missing variable is a visible error instead of silently broken markup.
        assert!(templates.render("page.html", json!({})).is_err());
    }

    #[test]
    fn bundled_templates_match_disk_and_escape_account_values() {
        let bundled = Templates::bundled().unwrap();
        let disk =
            Templates::Directory(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates"));
        let context = json!({"message":"<script>alert('x')</script>","assets":"<script src=\"/assets/main.js\"></script>"});
        let html = bundled.render("login.html", &context).unwrap();
        assert_eq!(html, disk.render("login.html", &context).unwrap());
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("<script src=\"/assets/main.js\"></script>"));
        // The public forms share included theme templates in both modes.
        for invite in [true, false] {
            let context = json!({"title":"Account","assets":"","invite":invite,"message":"","action":"/auth/test"});
            assert_eq!(
                bundled.render("public-account.html", &context).unwrap(),
                disk.render("public-account.html", &context).unwrap()
            );
        }
    }
}
