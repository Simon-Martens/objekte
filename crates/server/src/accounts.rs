//! Account administration uses Turso; token delivery is an explicit administrator action.
use crate::templates::Template;
use crate::{
    AppState,
    auth::Actor,
    error::{Result, WebError},
    routes, views,
};
use axum::{
    Router,
    extract::{Form, Path, State},
    http::HeaderMap,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use std::{collections::HashMap, sync::Arc};
type App = State<Arc<AppState>>;
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/account", get(account).post(profile))
        .route("/account/password", post(password))
        .route("/admin", get(admin))
        .route("/admin/{action}", post(admin_action))
        .route("/auth/invitations/{token}", get(invitation).post(redeem))
        .route("/auth/resets/{token}", get(reset).post(redeem_reset))
}
fn csrf(a: &Actor) -> String {
    format!("<input type=\"hidden\" name=\"csrf\" value=\"{}\">", a.csrf)
}
fn roles(current: &str) -> String {
    ["user", "editor", "administrator"]
        .iter()
        .map(|r| {
            format!(
                "<option {} value=\"{r}\">{}</option>",
                if *r == current { "selected" } else { "" },
                if *r == "user" { "reader" } else { r }
            )
        })
        .collect()
}
async fn account(State(state): App, headers: HeaderMap) -> Result<Response> {
    let a = match routes::page_actor(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    let body = format!(
        "<form method=\"post\" action=\"/account\">{}<label>Display name<input name=\"name\" value=\"{}\" required></label><button class=\"primary\"><i class=\"ri-save-line\" aria-hidden=\"true\"></i>Save profile</button></form><h2>Change password</h2><form method=\"post\" action=\"/account/password\">{}<label>Current password<input type=\"password\" name=\"current\" autocomplete=\"current-password\" required></label><label>New password<input type=\"password\" name=\"password\" autocomplete=\"new-password\" minlength=\"12\" required></label><button>Change password and sign out all sessions</button></form>",
        csrf(&a),
        views::escape(&a.name),
        csrf(&a)
    );
    routes::page(
        &state,
        &a,
        &headers,
        views::panel("/account", "Your account", &body)?,
    )
}
async fn profile(
    State(state): App,
    headers: HeaderMap,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Response> {
    let a = state.auth.actor(&headers).await?;
    state
        .auth
        .mutation(&a, f.get("csrf").map(String::as_str).unwrap_or(""), false)?;
    state
        .auth
        .profile(a.id, f.get("name").map(String::as_str).unwrap_or(""))
        .await?;
    Ok(Redirect::to("/account").into_response())
}
async fn password(
    State(state): App,
    headers: HeaderMap,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Response> {
    let a = state.auth.actor(&headers).await?;
    state
        .auth
        .mutation(&a, f.get("csrf").map(String::as_str).unwrap_or(""), false)?;
    state
        .auth
        .change_password(
            a.id,
            f.get("current").cloned().unwrap_or_default(),
            f.get("password").cloned().unwrap_or_default(),
        )
        .await?;
    state.sessions.lock().unwrap().logout(&a);
    Ok(Redirect::to("/auth/login").into_response())
}
fn qr_svg(url: &str) -> Result<String> {
    let code = qrcode::QrCode::new(url.as_bytes()).map_err(WebError::internal)?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(180, 180)
        .build())
}
fn absolute_link(headers: &HeaderMap, path: &str) -> Result<String> {
    let base = std::env::var("EDITOR_PUBLIC_URL")
        .ok()
        .or_else(|| {
            headers
                .get("origin")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| {
            format!(
                "http://{}",
                headers
                    .get("host")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("localhost:4322")
            )
        });
    let uri: axum::http::Uri = base
        .parse()
        .map_err(|_| WebError::bad("Invalid editor public URL"))?;
    if !matches!(uri.scheme_str(), Some("https" | "http"))
        || uri.host().is_none()
        || uri.path() != "/"
        || uri.query().is_some()
    {
        return Err(WebError::bad("Editor public URL must be an HTTP(S) origin"));
    }
    Ok(format!("{}{path}", base.trim_end_matches('/')))
}
async fn admin_html(
    state: &AppState,
    a: &Actor,
    message: &str,
    link: Option<&str>,
) -> Result<String> {
    state.auth.administrator(a)?;
    let users = state.auth.users().await?;
    let invitations = state.auth.invitations().await?;
    let mut body = format!(
        "<p role=\"status\">{}</p><div class=\"admin-overview\"><span><strong>{}</strong> users</span><span><strong>{}</strong> active invitations</span><span><i class=\"ri-shield-check-line\" aria-hidden=\"true\"></i>Role-based access</span></div>",
        views::escape(message),
        users.len(),
        invitations
            .iter()
            .filter(|i| !i.used && i.expires > crate::auth::now())
            .count()
    );
    if let Some(link) = link {
        body += &format!(
            "<section class=\"surface invitation-share\"><div class=\"qr-code\" role=\"img\" aria-label=\"QR code for the one-time account link\">{}</div><div><p class=\"eyebrow\">Ready to share</p><h2>Scan or copy the one-time link</h2><p>Deliver this link to the intended person. It is not emailed automatically.</p><div class=\"copy-link\"><input aria-label=\"One-time link\" readonly value=\"{}\"><button type=\"button\" data-copy=\"{}\"><i class=\"ri-file-copy-line\" aria-hidden=\"true\"></i>Copy link</button></div><a href=\"{}\">Open account link<i class=\"ri-arrow-right-up-line\" aria-hidden=\"true\"></i></a></div></section>",
            qr_svg(link)?,
            views::escape(link),
            views::escape(link),
            views::escape(link)
        );
    }
    body += "<div class=\"admin-columns\"><section><div class=\"section-heading standalone\"><h2><i class=\"ri-team-line\" aria-hidden=\"true\"></i>Users</h2></div><div class=\"user-grid\">";
    for u in users {
        body += &format!(
            "<section class=\"surface user-card\"><div class=\"user-card-heading\"><div class=\"user-identity\"><span class=\"user-avatar\">{}</span><div><h3>{}</h3><p class=\"mono\">{}</p></div></div><span class=\"badge\">{}</span></div><form class=\"user-controls\" method=\"post\" action=\"/admin/user\">{}<input type=\"hidden\" name=\"id\" value=\"{}\"><label>Role<select name=\"role\">{}</select></label><label class=\"checkbox-label\"><input type=\"checkbox\" name=\"disabled\" {}>Disable account</label><button class=\"small\">Update user</button></form><div class=\"user-card-footer\">",
            views::escape(&u.name.chars().take(2).collect::<String>().to_uppercase()),
            views::escape(&u.name),
            views::escape(&u.email),
            if u.disabled { "Disabled" } else { "Active" },
            csrf(a),
            u.id,
            roles(&u.role),
            if u.disabled { "checked" } else { "" }
        );
        for (action, label, icon) in [
            (
                "revoke-sessions",
                "Revoke sessions",
                "ri-logout-circle-line",
            ),
            ("reset", "Reset link", "ri-key-2-line"),
        ] {
            body += &format!(
                "<form method=\"post\" action=\"/admin/{action}\">{}<input type=\"hidden\" name=\"id\" value=\"{}\"><button class=\"small quiet\"><i class=\"{icon}\" aria-hidden=\"true\"></i>{label}</button></form>",
                csrf(a),
                u.id
            );
        }
        body += "</div></section>";
    }
    body += "</div></section><aside class=\"admin-invitations\"><section class=\"surface\"><div class=\"section-heading\"><h2><i class=\"ri-user-add-line\" aria-hidden=\"true\"></i>Invite a user</h2></div>";
    body += &format!(
        "<form class=\"invite-form\" method=\"post\" action=\"/admin/invite\">{}<p class=\"help\">Create a private invitation link and QR code. Invitations expire after seven days.</p><label>Email<input type=\"email\" name=\"email\" required placeholder=\"name@example.org\"></label><label>Role<select name=\"role\">{}</select></label><button class=\"primary\"><i class=\"ri-qr-code-line\" aria-hidden=\"true\"></i>Create invitation</button></form></section><section class=\"surface\"><div class=\"section-heading\"><h2><i class=\"ri-mail-send-line\" aria-hidden=\"true\"></i>Invitations</h2></div>",
        csrf(a),
        roles("user")
    );
    if invitations.is_empty() {
        body += "<div class=\"empty-state compact\"><p>No invitations yet.</p></div>";
    }
    for i in invitations {
        body += &format!(
            "<div class=\"invitation-card\"><strong>{}</strong><div><span class=\"badge\">{}</span><span class=\"help\">{}</span></div>",
            views::escape(&i.email),
            views::escape(&i.role),
            if i.used {
                "Used or revoked".into()
            } else if i.expires < crate::auth::now() {
                "Expired".into()
            } else {
                format!(
                    "{} days remaining",
                    (i.expires - crate::auth::now() + 86399) / 86400
                )
            }
        );
        if !i.used {
            body += &format!(
                "<form method=\"post\" action=\"/admin/revoke-invitation\">{}<input type=\"hidden\" name=\"id\" value=\"{}\"><button class=\"small quiet danger\">Revoke invitation</button></form>",
                csrf(a),
                i.id
            );
        }
        body += "</div>";
    }
    body += "</section></aside></div>";
    views::panel("/admin", "Users & access", &body)
}
async fn admin(State(state): App, headers: HeaderMap) -> Result<Response> {
    let a = match routes::page_actor(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    routes::page(
        &state,
        &a,
        &headers,
        admin_html(&state, &a, "", None).await?,
    )
}
async fn admin_action(
    State(state): App,
    Path(action): Path<String>,
    headers: HeaderMap,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Response> {
    let a = routes::mutation_actor(&state, &headers, &f).await?;
    state.auth.administrator(&a)?;
    let id = f.get("id").and_then(|s| s.parse().ok()).unwrap_or(0);
    let public_origin = if ["invite", "reset"].contains(&action.as_str()) {
        Some(absolute_link(&headers, "")?)
    } else {
        None
    };
    let mut link = None;
    match action.as_str() {
        "user" => {
            state
                .auth
                .set_user(
                    id,
                    f.get("role").map(String::as_str).unwrap_or(""),
                    f.contains_key("disabled"),
                )
                .await?
        }
        "revoke-sessions" => state.auth.revoke_sessions(id).await?,
        "invite" => {
            link = Some(format!(
                "/auth/invitations/{}",
                state
                    .auth
                    .invite(
                        f.get("email").map(String::as_str).unwrap_or(""),
                        f.get("role").map(String::as_str).unwrap_or("")
                    )
                    .await?
            ))
        }
        "revoke-invitation" => state.auth.revoke_invitation(id).await?,
        "reset" => {
            link = Some(format!(
                "/auth/resets/{}",
                state.auth.create_reset(id).await?
            ))
        }
        _ => return Err(WebError::bad("Unknown account operation")),
    };
    let link = link.map(|path| format!("{}{path}", public_origin.as_deref().unwrap_or("")));
    if id == a.id && ["user", "revoke-sessions"].contains(&action.as_str()) {
        return Ok(Redirect::to("/auth/login").into_response());
    }
    routes::page(
        &state,
        &a,
        &headers,
        admin_html(&state, &a, "Updated.", link.as_deref()).await?,
    )
}
crate::templates::template!(PublicAccount<'_>, "public-account.html");
#[derive(serde::Serialize)]
struct PublicAccount<'a> {
    title: &'a str,
    assets: &'a str,
    invite: bool,
    message: &'a str,
    action: &'a str,
}
fn public_form(state: &AppState, token: &str, invite: bool, message: &str) -> Result<Response> {
    let title = if invite {
        "Accept invitation"
    } else {
        "Reset password"
    };
    let body = PublicAccount {
        title,
        assets: &state.assets.tags,
        invite,
        message,
        action: &format!(
            "/auth/{}/{}",
            if invite { "invitations" } else { "resets" },
            views::encode(token)
        ),
    }
    .render()?;
    Ok(Html(body).into_response())
}
async fn invitation(State(s): App, Path(t): Path<String>) -> Result<Response> {
    public_form(&s, &t, true, "")
}
async fn reset(State(s): App, Path(t): Path<String>) -> Result<Response> {
    public_form(&s, &t, false, "")
}
async fn redeem(
    State(s): App,
    Path(t): Path<String>,
    headers: HeaderMap,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Response> {
    same_origin(&headers)?;
    match s
        .auth
        .redeem_invitation(
            &t,
            f.get("email").map(String::as_str).unwrap_or(""),
            f.get("name").map(String::as_str).unwrap_or(""),
            f.get("password").cloned().unwrap_or_default(),
        )
        .await
    {
        Ok(()) => Ok(Redirect::to("/auth/login").into_response()),
        Err(e) => public_form(&s, &t, true, &e.1),
    }
}
async fn redeem_reset(
    State(s): App,
    Path(t): Path<String>,
    headers: HeaderMap,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Response> {
    same_origin(&headers)?;
    match s
        .auth
        .redeem_reset(&t, f.get("password").cloned().unwrap_or_default())
        .await
    {
        Ok(()) => Ok(Redirect::to("/auth/login").into_response()),
        Err(e) => public_form(&s, &t, false, &e.1),
    }
}
fn same_origin(headers: &HeaderMap) -> Result<()> {
    if headers.get("sec-fetch-site").and_then(|s| s.to_str().ok()) == Some("cross-site") {
        Err(WebError::bad("Cross-site request refused"))
    } else {
        Ok(())
    }
}
