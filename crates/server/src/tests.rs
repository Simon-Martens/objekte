use super::*;
use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::json;
use std::collections::HashMap;
use tower::ServiceExt;

#[tokio::test]
async fn secure_cookies_are_only_disabled_explicitly() {
    let temp = tempfile::tempdir().unwrap();
    for insecure in [false, true] {
        let auth = Auth::open(
            &temp.path().join(format!("cookies-{insecure}.sqlite")),
            insecure,
        )
        .await
        .unwrap();
        for token in ["test-token", ""] {
            let cookie = auth.cookie(token);
            assert_eq!(cookie.contains("; Secure"), !insecure);
            assert!(cookie.contains("; HttpOnly; SameSite=Strict"));
            assert_eq!(cookie.contains("Max-Age=0"), token.is_empty());
        }
    }
}

async fn fixture() -> (tempfile::TempDir, Arc<AppState>, String, auth::Actor) {
    fixture_with_settings_schema(json!({"type":"object"})).await
}
async fn fixture_with_settings_schema(
    schema: serde_json::Value,
) -> (tempfile::TempDir, Arc<AppState>, String, auth::Actor) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    for dir in [".editor", "data/notes", "media"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    let files = [
        (
            ".editor/repository.json",
            json!({"objects":".editor/objects.schema.json","ui":".editor/ui.json","media":"media","settings":"settings.json","collections":{"notes":{"path":"data/notes","format":"json","id":"filename"}},"documents":{"editor-settings":{"schema":".editor/settings.schema.json","files":["settings.json"]}}}),
        ),
        (
            ".editor/objects.schema.json",
            json!({"anyOf":[{"x-collection":"notes","type":"object","required":["name","count"],"properties":{"name":{"type":"string","minLength":1},"count":{"type":"integer","minimum":0},"body":{"type":"string","x-content":"markdown"},"items":{"type":"array","items":{"type":"string"}}},"additionalProperties":false}]}),
        ),
        (
            ".editor/ui.json",
            json!({"collections":{"notes":{"label":"Notes","display":"name","fields":{"/properties/body":{"x-editor":"markdown"}}}}}),
        ),
        (".editor/settings.schema.json", schema),
        ("settings.json", json!({})),
        (
            "data/notes/one.json",
            json!({"name":"First","count":1,"body":"**Hello**","items":["a","b"]}),
        ),
    ];
    for (path, value) in files {
        std::fs::write(root.join(path), serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }
    let state = AppState::open(
        &root,
        &temp.path().join("state"),
        Assets {
            root: temp.path().join("assets"),
            tags: String::new(),
        },
        true,
    )
    .await
    .unwrap();
    state
        .auth
        .create_admin("test@example.test", "Test", "test-password-123".into())
        .await
        .unwrap();
    let token = state
        .auth
        .login("test@example.test", "test-password-123".into())
        .await
        .unwrap();
    let cookie = format!("objekte_session={token}");
    let mut headers = HeaderMap::new();
    headers.insert("cookie", cookie.parse().unwrap());
    let actor = state.auth.actor(&headers).await.unwrap();
    (temp, state, cookie, actor)
}
async fn request(
    state: Arc<AppState>,
    method: &str,
    url: &str,
    cookie: &str,
    form: &HashMap<String, String>,
    partial: bool,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(url)
        .header("cookie", cookie)
        .header("content-type", "application/x-www-form-urlencoded");
    if partial {
        request = request.header("datastar-request", "true");
    }
    let response = router(state)
        .oneshot(
            request
                .body(Body::from(serde_urlencoded::to_string(form).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    // Fragment failures use a successful SSE transport so Datastar can show them.
    let status = response
        .headers()
        .get("x-editor-error-status")
        .and_then(|value| StatusCode::from_bytes(value.as_bytes()).ok())
        .unwrap_or(response.status());
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    (status, body)
}
fn form(session: &sessions::EditSession, actor: &auth::Actor) -> HashMap<String, String> {
    let mut values = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), session.version.to_string()),
        ("record_id".into(), session.proposed_id.clone()),
    ]);
    fn fields(
        fields: &[structured_editor_core::draft::DraftField],
        values: &mut HashMap<String, String>,
    ) {
        for field in fields {
            if field.present {
                values.insert(format!("present.{}", field.id), "on".into());
            }
            values.insert(format!("value.{}", field.id), field.raw.clone());
            fields_iter(&field.items, values);
        }
    }
    fn fields_iter(
        items: &[structured_editor_core::draft::DraftField],
        values: &mut HashMap<String, String>,
    ) {
        fields(items, values)
    }
    fields(&session.draft.fields, &mut values);
    values
}
#[tokio::test]
async fn schema_revision_appears_in_validation_with_a_short_hash() {
    let (_temp, state, _cookie, actor) = fixture().await;
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let full_hash = "0123456789abcdef0123456789abcdef01234567";
    session
        .draft
        .fields
        .push(structured_editor_core::draft::DraftField {
            id: "system-metadata".into(),
            key: "_editor".into(),
            schema: json!({"type": "object"}),
            kind: structured_editor_core::draft::InputKind::Json,
            present: true,
            required: false,
            hidden: false,
            read_only: false,
            raw: json!({"schema_revision": full_hash}).to_string(),
            items: Vec::new(),
        });

    let html = views::edit(&state.editor, &session, &actor, &state.fields, &[], "").unwrap();
    let validation = html.find("Validation</h2>").unwrap();
    let revision = html
        .find("<div class=\"schema-revision\"><h3>Schema revision</h3>")
        .unwrap();
    assert!(validation < revision);
    assert!(!html[validation..revision].contains("</section>"));
    assert!(html.contains("<code>01234567</code>"));
    assert!(!html.contains(full_hash));
    assert_eq!(html.matches("Schema revision</h3>").count(), 1);
}
#[tokio::test]
async fn authentication_shell_and_fragment_contract() {
    let (_temp, state, cookie, actor) = fixture().await;
    let (status, _) = request(
        state.clone(),
        "GET",
        "/collections/notes",
        "",
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, full) = request(
        state.clone(),
        "GET",
        "/collections/notes",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(full.contains("id=\"admin-shell\""));
    assert!(full.contains("First"));
    let (_, fragment) = request(
        state.clone(),
        "GET",
        "/collections/notes",
        &cookie,
        &HashMap::new(),
        true,
    )
    .await;
    assert!(fragment.contains("event: datastar-patch-elements"));
    assert!(!fragment.contains("id=\"admin-shell\""));
    let (status, _) = request(
        state.clone(),
        "POST",
        "/collections/notes/records/one/edit-sessions",
        &cookie,
        &HashMap::new(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = request(
        state.clone(),
        "POST",
        "/collections/notes/records/one/edit-sessions",
        &cookie,
        &HashMap::from([("csrf".into(), actor.csrf.clone())]),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("record-form"));
    let mut reader = actor.clone();
    reader.role = "user".into();
    assert!(state.auth.mutation(&reader, &reader.csrf, true).is_err());
    state.auth.logout(&actor).await.unwrap();
    let (status, _) = request(
        state,
        "GET",
        "/collections/notes",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
}
#[tokio::test]
async fn invalid_input_stale_drafts_and_file_conflicts_preserve_data() {
    let (_temp, state, cookie, actor) = fixture().await;
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let url = format!("/edit-sessions/{}/save", session.id);
    let mut input = form(&session, &actor);
    let count = session
        .draft
        .fields
        .iter()
        .find(|f| f.key == "count")
        .unwrap();
    input.insert(format!("value.{}", count.id), "not a number".into());
    let (_, body) = request(state.clone(), "POST", &url, &cookie, &input, true).await;
    assert!(body.contains("Enter a valid integer"));
    assert!(body.contains("not a number"));
    assert_eq!(state.editor.get("notes", "one").unwrap().value["count"], 1);
    let (status, _) = request(state.clone(), "POST", &url, &cookie, &input, true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    let mut input = form(&current, &actor);
    input.insert(format!("value.{}", count.id), "2".into());
    std::fs::write(
        state.editor.config().root.join("data/notes/one.json"),
        r#"{"name":"external","count":7}"#,
    )
    .unwrap();
    let (_, body) = request(state.clone(), "POST", &url, &cookie, &input, true).await;
    assert!(body.contains("record changed on disk"));
    assert_eq!(state.editor.get("notes", "one").unwrap().value["count"], 7);
}
#[tokio::test]
async fn arrays_are_changed_by_server_and_session_is_owned() {
    let (_temp, state, cookie, actor) = fixture().await;
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let array = session
        .draft
        .fields
        .iter()
        .find(|f| f.key == "items")
        .unwrap();
    let item = &array.items[1].id;
    let url = format!(
        "/edit-sessions/{}/fields/{}/actions/up?item={item}",
        session.id, array.id
    );
    let (_, body) = request(
        state.clone(),
        "POST",
        &url,
        &cookie,
        &form(&session, &actor),
        true,
    )
    .await;
    assert!(body.contains("datastar-patch-elements"));
    let current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert_eq!(current.draft.value().unwrap()["items"], json!(["b", "a"]));
    assert_eq!(
        current
            .draft
            .fields
            .iter()
            .find(|f| f.key == "items")
            .unwrap()
            .items[0]
            .id,
        *item
    );
    let (_, body) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/save", session.id),
        &cookie,
        &form(&current, &actor),
        true,
    )
    .await;
    assert!(body.contains("Saved to disk"));
    assert_eq!(
        state.editor.get("notes", "one").unwrap().value["items"],
        json!(["b", "a"])
    );
    let token = state
        .auth
        .login("test@example.test", "test-password-123".into())
        .await
        .unwrap();
    let other = format!("objekte_session={token}");
    let (status, _) = request(
        state,
        "GET",
        &format!("/edit-sessions/{}", session.id),
        &other,
        &HashMap::new(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn templates_escape_record_content() {
    let (_temp, state, cookie, _) = fixture().await;
    let doc = state.editor.get("notes", "one").unwrap();
    state.editor.save("notes","one",json!({"name":"<script>alert(1)</script>","count":1,"body":"<script>alert(2)</script>"}),Some(&doc.revision)).unwrap();
    let (_, body) = request(
        state,
        "GET",
        "/collections/notes/records/one",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(!body.contains("<script>alert("));
    assert!(body.contains("&lt;script&gt;alert(1)"));
}

#[tokio::test]
async fn active_collection_search_offers_show_all() {
    let (_temp, state, cookie, _) = fixture().await;
    let (_, plain) = request(
        state.clone(),
        "GET",
        "/collections/notes",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(!plain.contains(">Show All</a>"));
    let (_, searched) = request(
        state,
        "GET",
        "/collections/notes?q=First&offset=50",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(searched.contains(">Show All</a>"));
    assert!(searched.contains("href=\"/collections/notes\""));
}

#[tokio::test]
async fn create_rename_and_delete_use_revision_checked_commands() {
    let (_temp, state, cookie, actor) = fixture().await;
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", None)
        .unwrap();
    let mut input = form(&session, &actor);
    input.insert("record_id".into(), "created".into());
    for field in &session.draft.fields {
        if let Some(value) = match field.key.as_str() {
            "name" => Some("Created"),
            "count" => Some("0"),
            _ => None,
        } {
            input.insert(format!("present.{}", field.id), "on".into());
            input.insert(format!("value.{}", field.id), value.into());
        }
    }
    let url = format!("/edit-sessions/{}/save", session.id);
    let (_, body) = request(state.clone(), "POST", &url, &cookie, &input, true).await;
    assert!(body.contains("Saved to disk"));
    assert_eq!(
        state.editor.get("notes", "created").unwrap().value,
        json!({"name":"Created","count":0})
    );
    let current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    let mut input = form(&current, &actor);
    input.insert("record_id".into(), "renamed".into());
    let (_, body) = request(state.clone(), "POST", &url, &cookie, &input, true).await;
    assert!(body.contains("ID changed. 0 referenced items updated."));
    assert!(state.editor.get("notes", "created").is_err());
    let saved = state.editor.get("notes", "renamed").unwrap();
    let mut deletion = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("revision".into(), "stale".into()),
    ]);
    let url = "/collections/notes/records/renamed/delete";
    let (status, _) = request(state.clone(), "POST", url, &cookie, &deletion, true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    deletion.insert("revision".into(), saved.revision);
    let (_, body) = request(state.clone(), "POST", url, &cookie, &deletion, true).await;
    assert!(body.contains("Record deleted"));
    assert!(body.contains("id=\"workspace\""));
    assert!(state.editor.get("notes", "renamed").is_err());
}

#[tokio::test]
async fn media_and_mapped_document_routes_enforce_auth_and_preserve_invalid_input() {
    let (_t, state, cookie, actor) = fixture().await;
    let csrf = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("path".into(), "".into()),
        ("name".into(), "scans".into()),
    ]);
    let (status, body) = request(
        state.clone(),
        "POST",
        "/media/media/actions/mkdir",
        &cookie,
        &csrf,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("scans"));
    let boundary = "objekte-test-upload";
    let bytes = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\n{}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"path\"\r\n\r\nscans\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\nUploaded content\r\n--{boundary}--\r\n",
        actor.csrf
    );
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/media/media/upload")
                .header("cookie", &cookie)
                .header("datastar-request", "true")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(bytes))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        state.editor.media_read("scans/test.txt").unwrap(),
        b"Uploaded content"
    );
    let (status, _) = request(
        state.clone(),
        "GET",
        "/media/media/file?path=scans%2Ftest.txt",
        "",
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, legacy) = request(
        state.clone(),
        "GET",
        "/api/media/file?path=scans%2Ftest.txt",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(legacy, "Uploaded content");
    let (status, refs) = request(
        state.clone(),
        "GET",
        "/media/media/references?path=scans%2Ftest.txt",
        &cookie,
        &HashMap::new(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(refs.contains("No explicit field references"));
    assert!(refs.contains("id=\"media-refs-"));
    assert!(!refs.contains("id=\"workspace\""));
    assert_eq!(
        request(
            state.clone(),
            "GET",
            "/media/media/references?path=scans%2Ftest.txt",
            "",
            &HashMap::new(),
            false
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );

    let d = state.editor.settings().unwrap();
    let mut f = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("revision".into(), d.revision),
        ("raw".into(), "{invalid".into()),
        ("action".into(), "save".into()),
    ]);
    let (_, body) = request(state.clone(), "POST", "/settings", &cookie, &f, true).await;
    assert!(body.contains("{invalid"));
    assert_eq!(state.editor.settings().unwrap().value, json!({}));
    f.insert("raw".into(), "{\"theme\":\"light\"}".into());
    let (_, body) = request(state.clone(), "POST", "/settings", &cookie, &f, true).await;
    assert!(body.contains("Saved to disk"));
    assert_eq!(state.editor.settings().unwrap().value["theme"], "light");
    f.insert("raw".into(), "{}".into());
    let (_, body) = request(state, "POST", "/settings", &cookie, &f, true).await;
    assert!(body.contains("changed on disk"));
}
#[tokio::test]
async fn account_tokens_are_single_use_roles_guard_routes_and_last_admin_is_preserved() {
    let (_t, state, _cookie, actor) = fixture().await;
    assert!(state.auth.set_user(actor.id, "user", false).await.is_err());
    let token = state
        .auth
        .invite("reader@example.test", "user")
        .await
        .unwrap();
    assert!(
        state
            .auth
            .redeem_invitation(
                &token,
                "other@example.test",
                "Reader",
                "reader-password-123".into()
            )
            .await
            .is_err()
    );
    state
        .auth
        .redeem_invitation(
            &token,
            "reader@example.test",
            "Reader",
            "reader-password-123".into(),
        )
        .await
        .unwrap();
    assert!(
        state
            .auth
            .redeem_invitation(
                &token,
                "reader@example.test",
                "Reader",
                "reader-password-123".into()
            )
            .await
            .is_err()
    );
    let login = state
        .auth
        .login("reader@example.test", "reader-password-123".into())
        .await
        .unwrap();
    let cookie = format!("objekte_session={login}");
    let mut headers = HeaderMap::new();
    headers.insert("cookie", cookie.parse().unwrap());
    let reader = state.auth.actor(&headers).await.unwrap();
    let (status, _) = request(
        state.clone(),
        "GET",
        "/admin",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = request(
        state.clone(),
        "POST",
        "/media/media/actions/mkdir",
        &cookie,
        &HashMap::from([
            ("csrf".into(), reader.csrf.clone()),
            ("name".into(), "forbidden".into()),
        ]),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let reset = state.auth.create_reset(reader.id).await.unwrap();
    state
        .auth
        .redeem_reset(&reset, "replacement-password-123".into())
        .await
        .unwrap();
    assert!(state.auth.actor(&headers).await.is_err());
    assert!(
        state
            .auth
            .redeem_reset(&reset, "second-password-123".into())
            .await
            .is_err()
    );
    assert!(
        state
            .auth
            .login("reader@example.test", "reader-password-123".into())
            .await
            .is_err()
    );
    let token = state
        .auth
        .login("reader@example.test", "replacement-password-123".into())
        .await
        .unwrap();
    headers.insert(
        "cookie",
        format!("objekte_session={token}").parse().unwrap(),
    );
    state
        .auth
        .change_password(
            reader.id,
            "replacement-password-123".into(),
            "another-password-123".into(),
        )
        .await
        .unwrap();
    assert!(state.auth.actor(&headers).await.is_err());
}
#[tokio::test]
async fn raw_drafts_and_markdown_link_commands_are_server_owned() {
    let (_t, state, cookie, actor) = fixture().await;
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let (_, body) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/raw", session.id),
        &cookie,
        &form(&session, &actor),
        true,
    )
    .await;
    assert!(body.contains("Entire record JSON"));
    let current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    let mut f = form(&current, &actor);
    f.insert("raw".into(), "{bad".into());
    let (_, body) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/save", session.id),
        &cookie,
        &f,
        true,
    )
    .await;
    assert!(body.contains("{bad"));
    assert_eq!(
        state.editor.get("notes", "one").unwrap().value["name"],
        "First"
    );
    let current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    let mut f = form(&current, &actor);
    f.insert(
        "raw".into(),
        json!({"name":"Raw","count":2,"body":"hello"}).to_string(),
    );
    request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/form", session.id),
        &cookie,
        &f,
        true,
    )
    .await;
    let current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    let field = current
        .draft
        .fields
        .iter()
        .find(|f| f.key == "body")
        .unwrap()
        .id
        .clone();
    let mut f = form(&current, &actor);
    f.insert(format!("selection_from.{field}"), "0".into());
    f.insert(format!("selection_to.{field}"), "5".into());
    let base = format!("/edit-sessions/{}/fields/{field}/actions", session.id);
    request(
        state.clone(),
        "POST",
        &format!("{base}/links"),
        &cookie,
        &f,
        true,
    )
    .await;
    let current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    let mut f = form(&current, &actor);
    f.insert("link_url".into(), "/collections/notes/records/one".into());
    f.insert("link_label".into(), "First".into());
    let (_, body) = request(
        state.clone(),
        "POST",
        &format!("{base}/insert-link"),
        &cookie,
        &f,
        true,
    )
    .await;
    assert!(body.contains("[First](/collections/notes/records/one)"));
    let current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert_eq!(
        current.value().unwrap()["body"],
        "[First](/collections/notes/records/one)"
    );
}

#[tokio::test]
async fn collection_configs_related_creation_and_generated_identity_work_together() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    fn copy(source: &std::path::Path, target: &std::path::Path) {
        std::fs::create_dir_all(target).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target.join(entry.file_name()));
            } else {
                std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
            }
        }
    }
    copy(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/project"),
        &root,
    );
    let ui_path = root.join(".editor/collections/articles/ui.json");
    let mut ui: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&ui_path).unwrap()).unwrap();
    ui["fields"]["/properties/author"]["x-create"] = json!({"nameField":"name"});
    std::fs::write(ui_path, ui.to_string()).unwrap();
    let state = AppState::open(
        &root,
        &temp.path().join("state"),
        Assets {
            root: temp.path().join("assets"),
            tags: String::new(),
        },
        true,
    )
    .await
    .unwrap();
    state
        .auth
        .create_admin("test@example.test", "Test", "test-password-123".into())
        .await
        .unwrap();
    let token = state
        .auth
        .login("test@example.test", "test-password-123".into())
        .await
        .unwrap();
    let cookie = format!("objekte_session={token}");
    let mut headers = HeaderMap::new();
    headers.insert("cookie", cookie.parse().unwrap());
    let actor = state.auth.actor(&headers).await.unwrap();
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "articles", None)
        .unwrap();
    assert!(
        session
            .draft
            .fields
            .iter()
            .find(|f| f.key == "id")
            .unwrap()
            .read_only
    );
    for (key, value) in [("title", "New article"), ("body", "Original parent body")] {
        let field = session
            .draft
            .fields
            .iter()
            .find(|f| f.key == key)
            .unwrap()
            .id
            .clone();
        session.draft.set_field(&field, json!(value)).unwrap();
    }
    let parent_id = session.id.clone();
    let author = session
        .draft
        .fields
        .iter()
        .find(|f| f.key == "author")
        .unwrap()
        .id
        .clone();
    state.sessions.lock().unwrap().insert(session.clone());
    let (status, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{parent_id}/generate-id"),
        &cookie,
        &form(&session, &actor),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("new-article"));
    session = state
        .sessions
        .lock()
        .unwrap()
        .get(&parent_id, &actor)
        .unwrap()
        .clone();
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{parent_id}/validate"),
        &cookie,
        &form(&session, &actor),
        true,
    )
    .await;
    assert!(html.contains("The draft is valid."), "{html}");
    session = state
        .sessions
        .lock()
        .unwrap()
        .get(&parent_id, &actor)
        .unwrap()
        .clone();
    request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{parent_id}/fields/{author}/actions/picker"),
        &cookie,
        &form(&session, &actor),
        true,
    )
    .await;
    session = state
        .sessions
        .lock()
        .unwrap()
        .get(&parent_id, &actor)
        .unwrap()
        .clone();
    let mut input = form(&session, &actor);
    input.insert("picker_query".into(), "Related person".into());
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{parent_id}/fields/{author}/actions/create-related"),
        &cookie,
        &input,
        true,
    )
    .await;
    let child_id = html
        .split("data-draft-id=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    assert_ne!(child_id, parent_id);
    let mut child = state
        .sessions
        .lock()
        .unwrap()
        .get(&child_id, &actor)
        .unwrap()
        .clone();
    let mut input = form(&child, &actor);
    input.insert("record_id".into(), "related-person".into());
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{child_id}/save"),
        &cookie,
        &input,
        true,
    )
    .await;
    assert!(
        html.contains("Related record created and selected"),
        "{html}"
    );
    child = state
        .sessions
        .lock()
        .unwrap()
        .get(&child_id, &actor)
        .unwrap()
        .clone();
    assert!(child.revision.is_some());
    session = state
        .sessions
        .lock()
        .unwrap()
        .get(&parent_id, &actor)
        .unwrap()
        .clone();
    assert_eq!(session.value().unwrap()["body"], "Original parent body");
    assert_eq!(session.value().unwrap()["author"], "related-person");
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{parent_id}/save"),
        &cookie,
        &form(&session, &actor),
        true,
    )
    .await;
    assert!(html.contains("Saved to disk"), "{html}");
    let doc = state.editor.get("articles", "new-article").unwrap();
    assert_eq!(doc.value["id"], "new-article");
    assert_eq!(doc.value["author"], "related-person");
    assert!(
        state
            .editor
            .delete("people", "related-person", &child.revision.unwrap())
            .is_err()
    );
}

#[tokio::test]
async fn suggestion_lookups_patch_only_options_without_mutating_the_draft() {
    let (_temp, state, cookie, actor) = fixture().await;
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let name = session
        .draft
        .fields
        .iter()
        .find(|f| f.key == "name")
        .unwrap()
        .id
        .clone();
    let mut input = form(&session, &actor);
    input.insert(format!("value.{name}"), "Fir".into());
    let (status, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/fields/{name}/suggestions", session.id),
        &cookie,
        &input,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("value=\"First\""));
    assert!(!html.contains("id=\"workspace\""));
    assert!(html.contains("data-query=\"Fir\""));
    input.insert(format!("value.{name}"), "No matching value".into());
    let (status, html) = request(
        state.clone(),
        "POST",
        &format!(
            "/edit-sessions/{}/fields/{name}/suggestions?all=1",
            session.id
        ),
        &cookie,
        &input,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("value=\"First\""));
    assert!(html.contains("data-query=\"\""));

    let current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert_eq!(current.version, session.version);
    assert_eq!(current.value().unwrap()["name"], "First");
}

#[tokio::test]
async fn live_validation_returns_only_feedback_and_never_changes_draft_or_disk() {
    let (_temp, state, cookie, actor) = fixture().await;
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let count = session
        .draft
        .fields
        .iter()
        .find(|f| f.key == "count")
        .unwrap()
        .id
        .clone();
    assert!(
        crate::views::draft_exit(&session)
            .unwrap()
            .contains(">Close</button>")
    );
    let url = format!("/edit-sessions/{}/live-validation", session.id);
    let mut values = form(&session, &actor);
    values.insert(format!("value.{count}"), "-3".into());
    let (status, body) = request(state.clone(), "POST", &url, &cookie, &values, true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(&format!("id=\"errors-{count}\"")));
    assert!(body.contains("/count"));
    assert!(body.contains("issue(s) to resolve"));
    assert!(body.contains(">Discard</button>"));
    assert!(!body.contains("id=\"workspace\""));
    assert!(!body.contains("code-input"));
    assert!(!body.contains("<input"));
    assert_eq!(
        state
            .sessions
            .lock()
            .unwrap()
            .get(&session.id, &actor)
            .unwrap()
            .version,
        session.version
    );
    assert_eq!(state.editor.get("notes", "one").unwrap().value["count"], 1);
    values.insert(format!("value.{count}"), "0".into());
    let (_, body) = request(state.clone(), "POST", &url, &cookie, &values, true).await;
    assert!(body.contains("All fields are valid"));
    assert!(body.contains(&format!(
        "id=\"errors-{count}\" class=\"field-errors\" aria-live=\"polite\"></div>"
    )));
    values.insert(format!("value.{count}"), "1".into());
    let (_, body) = request(state.clone(), "POST", &url, &cookie, &values, true).await;
    assert!(body.contains(">Close</button>"));
    assert!(!body.contains(">Discard</button>"));
    values.insert("record_id".into(), "invalid/id".into());
    let (_, body) = request(state.clone(), "POST", &url, &cookie, &values, true).await;
    assert!(body.contains("/id"));
    values.insert("version".into(), "9999".into());
    assert_eq!(
        request(state.clone(), "POST", &url, &cookie, &values, true)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    values.remove("csrf");
    assert_eq!(
        request(state, "POST", &url, &cookie, &values, true).await.0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn document_live_validation_does_not_save_or_replace_the_source() {
    let (_temp, state, cookie, actor) = fixture().await;
    let mut values = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("raw".into(), "{ broken".into()),
    ]);
    let (status, body) = request(
        state.clone(),
        "POST",
        "/settings/validation",
        &cookie,
        &values,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("document-validation"));
    assert!(body.contains("invalid-text"));
    assert!(!body.contains("<textarea"));
    assert_eq!(state.editor.settings().unwrap().value, json!({}));
    values.insert("raw".into(), "{\"draft\":true}".into());
    let (_, body) = request(
        state.clone(),
        "POST",
        "/documents/editor-settings/validation",
        &cookie,
        &values,
        true,
    )
    .await;
    assert!(body.contains("all schema checks passed"));
    assert_eq!(state.editor.settings().unwrap().value, json!({}));
    values.remove("csrf");
    assert_eq!(
        request(
            state,
            "POST",
            "/settings/validation",
            &cookie,
            &values,
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn field_removal_undo_redo_and_live_preview_preserve_raw_input() {
    let (_temp, state, cookie, actor) = fixture().await;
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let field = session
        .draft
        .fields
        .iter()
        .find(|f| f.key == "body")
        .unwrap()
        .id
        .clone();
    let base = format!("/edit-sessions/{}", session.id);
    let mut values = form(&session, &actor);
    values.insert(format!("value.{field}"), "**Edited**\n\ntext".into());
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("{base}/fields/{field}/actions/omit"),
        &cookie,
        &values,
        true,
    )
    .await;
    assert!(!html.contains(&format!("id=\"code-{field}\"")));
    assert!(html.contains("Add field"));
    assert!(!html.contains("Include field"));
    let mut current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert!(!current.draft.field(&field).unwrap().present);
    let (_, restored) = request(
        state.clone(),
        "POST",
        &format!("{base}/undo"),
        &cookie,
        &form(&current, &actor),
        true,
    )
    .await;
    assert!(restored.contains("data: mode replace\n"));
    current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert_eq!(current.value().unwrap()["body"], "**Edited**\n\ntext");
    request(
        state.clone(),
        "POST",
        &format!("{base}/redo"),
        &cookie,
        &form(&current, &actor),
        true,
    )
    .await;
    current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert!(!current.draft.field(&field).unwrap().present);
    request(
        state.clone(),
        "POST",
        &format!("{base}/undo"),
        &cookie,
        &form(&current, &actor),
        true,
    )
    .await;
    current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    request(
        state.clone(),
        "POST",
        &format!("{base}/fields/{field}/actions/preview"),
        &cookie,
        &form(&current, &actor),
        true,
    )
    .await;
    current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    let version = current.version;
    let mut values = form(&current, &actor);
    values.insert(format!("value.{field}"), "Live **preview**".into());
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("{base}/live-validation"),
        &cookie,
        &values,
        true,
    )
    .await;
    assert!(html.contains("Live <strong>preview</strong>"));
    assert!(!html.contains("<textarea"));
    assert_eq!(
        state
            .sessions
            .lock()
            .unwrap()
            .get(&session.id, &actor)
            .unwrap()
            .version,
        version
    );
    request(
        state.clone(),
        "POST",
        &format!("{base}/fields/{field}/actions/preview"),
        &cookie,
        &values,
        true,
    )
    .await;
    assert!(
        state
            .sessions
            .lock()
            .unwrap()
            .get(&session.id, &actor)
            .unwrap()
            .preview
            .is_empty()
    );
    assert_eq!(
        state.editor.get("notes", "one").unwrap().value["body"],
        "**Hello**"
    );
}
#[test]
fn search_highlights_unicode_without_interpreting_markup() {
    let html = views::list::highlighted(
        "<script>İstanbul & HERDER</script>",
        &structured_editor_core::SearchMatcher::new("herder", Default::default())
            .unwrap()
            .ranges("<script>İstanbul & HERDER</script>"),
        true,
    );
    assert!(html.contains("<mark>HERDER</mark>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>"));
    assert_eq!(
        views::list::highlighted(
            "Über",
            &structured_editor_core::SearchMatcher::new("über", Default::default())
                .unwrap()
                .ranges("Über"),
            false
        ),
        "<mark>Über</mark>"
    );
}

#[tokio::test]
async fn invitations_render_a_local_qr_and_a_reviewable_share_link() {
    let (_temp, state, cookie, actor) = fixture().await;
    let f = HashMap::from([
        ("csrf".into(), actor.csrf),
        ("email".into(), "invited@example.test".into()),
        ("role".into(), "user".into()),
    ]);
    let (status, html) = request(state, "POST", "/admin/invite", &cookie, &f, true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("<svg"));
    assert!(html.contains("QR code for the one-time account link"));
    assert!(html.contains("http://localhost:4322/auth/invitations/"));
    assert!(html.contains("user-grid"));
    assert!(html.contains("admin-invitations"));
}

#[tokio::test]
async fn generated_settings_preserve_unknown_keys_and_reject_invalid_changes() {
    let (_temp, state, cookie, actor) = fixture_with_settings_schema(json!({
        "type":"object", "properties": {
            "pageSize":{"type":"integer","minimum":10,"maximum":200,"default":50},
            "defaultTheme":{"type":"string","enum":["system","light","dark"],"default":"system"},
            "markdownPreview":{"type":"boolean","default":false}
        }
    }))
    .await;
    let original = state.editor.settings().unwrap();
    let saved = state
        .editor
        .save_settings(
            json!({"external":{"keep":"me"},"markdownPreview":true}),
            &original.revision,
        )
        .unwrap();
    let mut values = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("settings_form".into(), "1".into()),
        ("revision".into(), saved.revision),
        ("setting.pageSize".into(), "25".into()),
        ("setting.defaultTheme".into(), "\"dark\"".into()),
    ]);
    let (_, html) = request(state.clone(), "POST", "/settings", &cookie, &values, true).await;
    assert!(html.contains("Saved to disk"), "{html}");
    let saved = state.editor.settings().unwrap();
    assert_eq!(
        saved.value,
        json!({"external":{"keep":"me"},"markdownPreview":false,"defaultTheme":"dark","pageSize":25})
    );
    values.insert("revision".into(), saved.revision.clone());
    values.insert("setting.pageSize".into(), "201".into());
    let (_, feedback) = request(
        state.clone(),
        "POST",
        "/settings/validation",
        &cookie,
        &values,
        true,
    )
    .await;
    assert!(feedback.contains("pageSize"));
    assert!(!feedback.contains("<input"));
    let (_, html) = request(state.clone(), "POST", "/settings", &cookie, &values, true).await;
    assert!(html.contains("value=\"201\""));
    assert!(!html.contains("Saved to disk"));
    assert_eq!(state.editor.settings().unwrap().revision, saved.revision);
    values.insert("setting.pageSize".into(), "25.5".into());
    let (_, html) = request(state.clone(), "POST", "/settings", &cookie, &values, true).await;
    assert!(html.contains("value=\"25.5\""), "{html}");
    assert!(html.contains("data-notification=\"error\""), "{html}");
    assert_eq!(state.editor.settings().unwrap().revision, saved.revision);
}

#[tokio::test]
async fn media_replacement_http_preserves_the_path_and_checks_revision_and_format() {
    let (_temp, state, cookie, actor) = fixture().await;
    state
        .editor
        .media_upload("scan.txt", b"Original", 100)
        .unwrap();
    let revision = state.editor.media_revision("scan.txt").unwrap();
    for (filename, rev, csrf, expected) in [
        (
            "new.png",
            revision.as_str(),
            actor.csrf.as_str(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "new.txt",
            "stale",
            actor.csrf.as_str(),
            StatusCode::CONFLICT,
        ),
        (
            "new.txt",
            revision.as_str(),
            "invalid",
            StatusCode::FORBIDDEN,
        ),
        (
            "new.txt",
            revision.as_str(),
            actor.csrf.as_str(),
            StatusCode::OK,
        ),
    ] {
        let boundary = "objekte-replace-test";
        let bytes = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\n{csrf}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"replacement\"\r\n\r\nscan.txt\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"revision\"\r\n\r\n{rev}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: text/plain\r\n\r\nReplacement\r\n--{boundary}--\r\n"
        );
        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/media/media/upload")
                    .header("cookie", &cookie)
                    .header("datastar-request", "true")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(bytes))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-editor-error-status")
                .and_then(|value| StatusCode::from_bytes(value.as_bytes()).ok())
                .unwrap_or(StatusCode::OK),
            expected
        );
        assert_eq!(
            state.editor.media_read("scan.txt").unwrap(),
            if expected == StatusCode::OK {
                b"Replacement".as_slice()
            } else {
                b"Original".as_slice()
            }
        );
        assert!(state.editor.media_read(filename).is_err());
    }
}

async fn hierarchy_fixture() -> (tempfile::TempDir, Arc<AppState>, String, auth::Actor) {
    let (temp, mut state, cookie, actor) = fixture().await;
    let root = temp.path().join("project");
    let load = |path: &str| {
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(root.join(path)).unwrap())
            .unwrap()
    };
    let write = |path: &str, value: &serde_json::Value| {
        std::fs::write(root.join(path), serde_json::to_vec_pretty(value).unwrap()).unwrap()
    };
    let mut config = load(".editor/repository.json");
    let mut schemas = load(".editor/objects.schema.json");
    let mut ui = load(".editor/ui.json");
    for (name, target) in [("places", "groups"), ("groups", "notes")] {
        std::fs::create_dir(root.join(format!("data/{name}"))).unwrap();
        config["collections"][name] =
            json!({"path":format!("data/{name}"),"format":"json","id":"filename"});
        schemas["anyOf"].as_array_mut().unwrap().push(json!({"x-collection":name,"type":"object","required":["name"],"properties":{"name":{"type":"string"},"children":{"type":"array","uniqueItems":true,"x-children":true,"items":{"type":"string","x-relation":{"collection":target}}}},"additionalProperties":false}));
        ui["collections"][name] = json!({
            "label": name,
            "display": "name",
            "showInSidebar": name == "places",
            "sidebarAllLinks": "root"
        });
    }
    write(".editor/repository.json", &config);
    write(".editor/objects.schema.json", &schemas);
    write(".editor/ui.json", &ui);
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    state
        .editor
        .save(
            "groups",
            "g",
            json!({"name":"Group","children":["one"]}),
            None,
        )
        .unwrap();
    state
        .editor
        .save(
            "places",
            "p",
            json!({"name":"Place","children":["g"]}),
            None,
        )
        .unwrap();
    (temp, state, cookie, actor)
}

#[tokio::test]
async fn hierarchy_routes_render_scope_create_attach_and_reject_stale_changes() {
    let (_temp, state, cookie, actor) = hierarchy_fixture().await;
    let g = state.editor.get("groups", "g").unwrap();
    let (_, html) = request(
        state.clone(),
        "GET",
        "/collections/places/records/p",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(html.contains("id=\"children-children\""));
    assert!(html.contains("New record"));
    assert!(html.contains("Link existing"));
    assert!(html.contains("data-active-branch=\"true\""));
    assert!(html.contains("<a class=\"tree-all\" data-nav href=\"/collections/places\""));
    assert!(!html.contains("data-nav class=\"tree-all\" href=\"/collections/places/records/p\""));
    assert!(!html.contains("data-nav class=\"tree-all\" href=\"/collections/groups/records/g\""));
    assert!(
        hierarchy::navigation_tree(&state.editor, "places", "", true)
            .unwrap()
            .contains("data-nav class=\"tree-all\" href=\"/collections/groups/records/g\"")
    );
    assert!(!html.contains("All in "));
    let containers = hierarchy::navigation_tree(
        &state.editor,
        "places",
        "data-record-url=\"/collections/notes/records/one\"",
        false,
    )
    .unwrap();
    assert!(!containers.contains("tree-all"));
    assert!(!containers.contains("href=\"/collections/notes/records/one\""));
    assert!(containers.contains("href=\"/collections/groups/records/g\" aria-current=\"page\""));
    assert!(
        !containers.contains("<details class=\"tree-node\" id=\"nav-places-places-p-groups-g\"")
    );
    let base = "/collections/groups/records/g/children/children";
    let mut mutation = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("revision".into(), g.revision.clone()),
        ("child_id".into(), "one".into()),
    ]);
    mutation.insert("csrf".into(), "invalid".into());
    assert_eq!(
        request(
            state.clone(),
            "POST",
            &format!("{base}/unlink"),
            &cookie,
            &mutation,
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    mutation.insert("csrf".into(), actor.csrf.clone());
    assert_eq!(
        request(
            state.clone(),
            "POST",
            &format!("{base}/unlink"),
            &cookie,
            &mutation,
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(state.editor.get("notes", "one").is_ok());
    assert_eq!(
        request(
            state.clone(),
            "POST",
            &format!("{base}/link"),
            &cookie,
            &mutation,
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, picker) = request(
        state.clone(),
        "GET",
        &format!("{base}/picker?q=First"),
        &cookie,
        &HashMap::new(),
        true,
    )
    .await;
    assert!(picker.contains("First") && picker.contains("name=\"child_id\" value=\"one\""));
    let g = state.editor.get("groups", "g").unwrap();
    mutation.insert("revision".into(), g.revision.clone());
    assert_eq!(
        request(
            state.clone(),
            "POST",
            &format!("{base}/link"),
            &cookie,
            &mutation,
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    let scope = hierarchy::scoped_url("groups", "g", "children", "notes");
    let (_, html) = request(state.clone(), "GET", &scope, &cookie, &HashMap::new(), true).await;
    assert!(html.contains("First"));
    assert!(html.contains("name=\"parent_field\" value=\"children\""));
    let bad = "/collections/notes?parent_collection=groups";
    assert_eq!(
        request(state.clone(), "GET", bad, &cookie, &HashMap::new(), true)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let g = state.editor.get("groups", "g").unwrap();
    let context = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("parent_collection".into(), "groups".into()),
        ("parent_id".into(), "g".into()),
        ("parent_field".into(), "children".into()),
        ("parent_revision".into(), g.revision.clone()),
    ]);
    let (_, html) = request(
        state.clone(),
        "POST",
        "/collections/notes/edit-sessions",
        &cookie,
        &context,
        true,
    )
    .await;
    let id = html
        .split("data-draft-id=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let session = state
        .sessions
        .lock()
        .unwrap()
        .get(id, &actor)
        .unwrap()
        .clone();
    let mut values = form(&session, &actor);
    values.insert("record_id".into(), "created-child".into());
    for f in &session.draft.fields {
        if let Some(value) = match f.key.as_str() {
            "name" => Some("Child"),
            "count" => Some("0"),
            _ => None,
        } {
            values.insert(format!("present.{}", f.id), "on".into());
            values.insert(format!("value.{}", f.id), value.into());
        }
    }
    let (status, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{id}/save"),
        &cookie,
        &values,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("data-url=\"/collections/notes/records/created%2Dchild/edit\""));
    assert_eq!(
        state
            .editor
            .child_ids("groups", "g", "children")
            .unwrap()
            .len(),
        2
    );
    assert!(state.editor.get("notes", "created-child").is_ok());
}

#[tokio::test]
async fn deletion_review_resolves_placements_but_preserves_other_references_and_revisions() {
    let (_temp, state, cookie, actor) = hierarchy_fixture().await;
    let target = state.editor.get("notes", "one").unwrap();
    state.editor.save("notes", "referrer", json!({"name":"Linked note","count":0,"body":"[First](/collections/notes/records/one)"}), None).unwrap();
    let url = "/collections/notes/records/one/delete";
    let (_, html) = request(state.clone(), "GET", url, &cookie, &HashMap::new(), true).await;
    assert!(
        html.contains("List placements")
            && html.contains("Group")
            && html.contains("Other references")
    );
    assert!(html.contains("Linked note") && html.contains("Unlink from this list"));
    assert!(html.contains("class=\"danger\" disabled"));
    let delete = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("revision".into(), target.revision.clone()),
    ]);
    let (status, html) = request(state.clone(), "POST", url, &cookie, &delete, true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(html.contains("delete-review"));
    assert_eq!(
        state.editor.get("notes", "one").unwrap().revision,
        target.revision
    );

    let group = state.editor.get("groups", "g").unwrap();
    let unlink_url = format!("{url}/unlink");
    let mut unlink = HashMap::from([
        ("csrf".into(), "invalid".into()),
        ("revision".into(), group.revision.clone()),
        ("parent_collection".into(), "groups".into()),
        ("parent_id".into(), "g".into()),
        ("parent_field".into(), "children".into()),
    ]);
    assert_eq!(
        request(state.clone(), "POST", &unlink_url, &cookie, &unlink, true)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    unlink.insert("csrf".into(), actor.csrf.clone());
    unlink.insert("parent_collection".into(), "places".into());
    unlink.insert("parent_id".into(), "p".into());
    assert_eq!(
        request(state.clone(), "POST", &unlink_url, &cookie, &unlink, true)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    unlink.insert("parent_collection".into(), "groups".into());
    unlink.insert("parent_id".into(), "g".into());
    unlink.insert("revision".into(), "stale".into());
    assert_eq!(
        request(state.clone(), "POST", &unlink_url, &cookie, &unlink, true)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        state.editor.get("groups", "g").unwrap().revision,
        group.revision
    );
    unlink.insert("revision".into(), group.revision);
    let (status, html) = request(state.clone(), "POST", &unlink_url, &cookie, &unlink, true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Other references") && !html.contains("List placements"));
    assert!(html.contains("class=\"danger\" disabled"));
    assert!(
        state
            .editor
            .child_ids("groups", "g", "children")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        state.editor.get("notes", "one").unwrap().revision,
        target.revision
    );

    let other = state.editor.get("notes", "referrer").unwrap();
    state
        .editor
        .save(
            "notes",
            "referrer",
            json!({"name":"Linked note","count":0}),
            Some(&other.revision),
        )
        .unwrap();
    let (_, html) = request(state.clone(), "GET", url, &cookie, &HashMap::new(), true).await;
    assert!(html.contains("No incoming references remain"));
    assert!(!html.contains("class=\"danger\" disabled"));
    let (status, html) = request(state.clone(), "POST", url, &cookie, &delete, true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Record deleted"));
    assert!(state.editor.get("notes", "one").is_err());
    assert!(state.editor.get("notes", "referrer").is_ok());
}

#[tokio::test]
async fn fragment_errors_are_delivered_as_visible_patches() {
    let (_temp, state, cookie, _) = fixture().await;
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/collections/notes/records/one/delete")
                .header("cookie", &cookie)
                .header("datastar-request", "true")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("csrf=invalid&revision=stale"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-editor-error-status"], "403");
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("event: datastar-patch-elements"));
    assert!(body.contains("id=\"notification-region\" role=\"alert\""));
    // Router/extractor rejections have no typed WebError, but must still be visible.
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/unavailable-action")
                .header("cookie", &cookie)
                .header("datastar-request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-editor-error-status"], "404");
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("This page or action is no longer available"));
    assert_eq!(
        request(
            state,
            "POST",
            "/collections/notes/records/one/delete",
            &cookie,
            &HashMap::new(),
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

async fn field_types_fixture() -> (tempfile::TempDir, Arc<AppState>, String, auth::Actor) {
    let (temp, mut state, cookie, actor) = fixture().await;
    let root = temp.path().join("project");
    let mut schema: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/field-types.schema.json")).unwrap();
    schema["x-collection"] = json!("notes");
    std::fs::write(
        root.join(".editor/objects.schema.json"),
        json!({"anyOf":[schema]}).to_string(),
    )
    .unwrap();
    std::fs::write(root.join(".editor/ui.json"), json!({"collections":{"notes":{"label":"Notes","display":"title","list":{"columns":["title","count"],"filters":["enabled"]}}}}).to_string()).unwrap();
    std::fs::write(
        root.join("data/notes/one.json"),
        include_str!("../tests/fixtures/field-types.json"),
    )
    .unwrap();
    std::fs::copy(
        root.join("data/notes/one.json"),
        root.join("data/notes/two.json"),
    )
    .unwrap();
    std::fs::write(
        root.join("settings.json"),
        json!({"pageSize":1}).to_string(),
    )
    .unwrap();
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    (temp, state, cookie, actor)
}

#[tokio::test]
async fn record_view_renders_typed_array_items_and_escapes_source() {
    let (_temp, state, cookie, _) = field_types_fixture().await;
    let (status, html) = request(
        state,
        "GET",
        "/collections/notes/records/one",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("<ol class=\"preview-array\"><li><p>First <strong>bold</strong> note</p>"),
        "{html}"
    );
    assert!(html.contains("Second <em>italic</em> note"));
    assert!(html.contains("<li><ol class=\"preview-array\"><li><span class=\"mono\">1</span>"));
    assert!(html.contains("<span class=\"preview-text\">Line one\nLine two</span>"));
    assert!(html.contains("<span>No</span>"));
    assert!(html.contains("<span class=\"mono\">0</span>"));
    assert!(html.contains("<pre class=\"preview-source\">null</pre>"));
    assert!(html.contains("No items"));
    assert!(html.contains("2026-10-23T14:30:00+02:00"));
    assert!(html.contains("href=\"/collections/notes/records/one\">Field review</a>"));
    assert!(html.contains("/media/media/file?path=sample%2Ejpg"));
    assert!(html.contains("&lt;script&gt;alert(&quot;source only&quot;)&lt;/script&gt;"));
    assert!(!html.contains("<script>alert"));
    assert!(!html.contains("<strong>plain JSON</strong>"));
}

#[tokio::test]
async fn record_view_names_linked_agents_inside_arrays() {
    let (temp, mut state, cookie, _) = fixture().await;
    let root = temp.path().join("project");
    let repository = root.join(".editor/repository.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&repository).unwrap()).unwrap();
    config["collections"]["agents"] = json!({"path":"data/agents","format":"json","id":"filename"});
    std::fs::write(&repository, config.to_string()).unwrap();
    let schema_path = root.join(".editor/objects.schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&schema_path).unwrap()).unwrap();
    schema["anyOf"][0]["properties"]["agents"] = json!({"type":"array","items":{"type":"object","properties":{"agent":{"type":"string","x-relation":{"collection":"agents"}},"type":{"type":"string"}},"required":["agent"],"additionalProperties":false}});
    schema["anyOf"].as_array_mut().unwrap().push(json!({"x-collection":"agents","type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}));
    std::fs::write(schema_path, schema.to_string()).unwrap();
    let ui_path = root.join(".editor/ui.json");
    let mut ui: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&ui_path).unwrap()).unwrap();
    ui["collections"]["agents"] = json!({"label":"Agents","display":"name"});
    std::fs::write(ui_path, ui.to_string()).unwrap();
    std::fs::create_dir_all(root.join("data/agents")).unwrap();
    std::fs::write(
        root.join("data/agents/ada.json"),
        json!({"name":"Ada & Co"}).to_string(),
    )
    .unwrap();
    let note_path = root.join("data/notes/one.json");
    let mut note: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&note_path).unwrap()).unwrap();
    note["agents"] = json!([{"agent":"ada","type":"Autor:in"}]);
    std::fs::write(note_path, note.to_string()).unwrap();
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    let (status, html) = request(
        state.clone(),
        "GET",
        "/collections/notes/records/one?node=%2Fagents",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Open Ada &amp; Co"), "{html}");
    let (status, html) = request(
        state,
        "GET",
        "/collections/notes/records/one?node=%2Fagents%2F0",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("/collections/agents/records/ada\">Ada &amp; Co</a>"),
        "{html}"
    );
}

#[tokio::test]
async fn all_field_controls_round_trip_and_markdown_arrays_keep_order() {
    let (_temp, state, cookie, actor) = field_types_fixture().await;
    let original = state.editor.get("notes", "one").unwrap().value;
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let fields = session
        .draft
        .fields
        .iter()
        .map(|f| (f.key.clone(), f.id.clone()))
        .collect::<HashMap<_, _>>();
    let (_, html) = request(
        state.clone(),
        "GET",
        &format!("/edit-sessions/{}", session.id),
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(html.contains("inputmode=\"numeric\""));
    assert!(html.contains("inputmode=\"decimal\""));
    assert!(html.contains("class=\"text-multiline\""));
    assert!(html.contains("aria-describedby=\"errors-"));
    assert!(html.contains("aria-label=\"Reorder item 1\"") && html.contains("data-down="));
    assert!(!html.contains("ITEM 1"));
    assert!(session.draft.field(&fields["locked"]).unwrap().read_only);
    let array = session.draft.field(&fields["notes"]).unwrap();
    let (status, _) = request(
        state.clone(),
        "POST",
        &format!(
            "/edit-sessions/{}/fields/{}/actions/down?item={}",
            session.id, array.id, array.items[0].id
        ),
        &cookie,
        &form(&session, &actor),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let session = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    let (status, _) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/save", session.id),
        &cookie,
        &form(&session, &actor),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let saved = state.editor.get("notes", "one").unwrap().value;
    for (key, value) in original.as_object().unwrap() {
        if key != "notes" {
            assert_eq!(&saved[key], value, "field {key} changed");
        }
    }
    assert_eq!(
        saved["notes"],
        json!([original["notes"][1], original["notes"][0]])
    );
    assert!(saved.get("optional").is_none());
}

#[tokio::test]
async fn custom_list_columns_survive_search_sort_filters_and_pagination() {
    let (_temp, state, cookie, _) = field_types_fixture().await;
    let url = "/collections/notes?columns=custom&column.title=1&column.enabled=1&q=Field&filter.enabled=false";
    let (status, html) = request(state.clone(), "GET", url, &cookie, &HashMap::new(), true).await;
    assert_eq!(status, StatusCode::OK);
    let headings = html
        .split("<thead>")
        .nth(1)
        .unwrap()
        .split("</thead>")
        .next()
        .unwrap();
    assert!(headings.contains("Title"));
    assert!(headings.contains("Enabled"));
    assert!(!headings.contains("Count"));
    assert!(html.contains("data-on:change=\"el.requestSubmit()\""));
    assert!(html.contains("column%2Eenabled=1"));
    assert!(html.contains("columns=custom"));
    assert!(html.contains("filter%2Eenabled=false"));
    assert!(html.contains("offset=1"));
    let (_, second) = request(
        state,
        "GET",
        &format!("{url}&offset=1&sort=title&direction=desc"),
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(second.contains("aria-sort=\"descending\""));
    assert!(second.contains("data-label=\"Enabled\""));
    assert!(!second.contains("data-label=\"Count\""));
}

#[tokio::test]
async fn added_array_slots_render_without_defaults_and_accept_typed_input() {
    let (_temp, state, cookie, actor) = field_types_fixture().await;
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    for (key, raw, expected) in [
        ("notes", "Added **note**", json!("Added **note**")),
        ("empty", "false", json!(false)),
    ] {
        let array = session
            .draft
            .fields
            .iter()
            .find(|f| f.key == key)
            .unwrap()
            .id
            .clone();
        let (status, html) = request(
            state.clone(),
            "POST",
            &format!("/edit-sessions/{}/fields/{array}/actions/add", session.id),
            &cookie,
            &form(&session, &actor),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        session = state
            .sessions
            .lock()
            .unwrap()
            .get(&session.id, &actor)
            .unwrap()
            .clone();
        let item = session.draft.field(&array).unwrap().items.last().unwrap();
        assert!(!item.present, "No default should be invented in the draft");
        assert!(
            html.contains(&format!("name=\"value.{}\"", item.id)),
            "New item has no editable control"
        );
        assert!(html.contains(&format!("name=\"present.{}\"", item.id)));
        if key == "empty" {
            assert!(html.contains("Choose a value"));
        }
        let mut values = form(&session, &actor);
        values.insert(format!("present.{}", item.id), "1".into());
        values.insert(format!("value.{}", item.id), raw.into());
        let (status, _) = request(
            state.clone(),
            "POST",
            &format!("/edit-sessions/{}/validate", session.id),
            &cookie,
            &values,
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        session = state
            .sessions
            .lock()
            .unwrap()
            .get(&session.id, &actor)
            .unwrap()
            .clone();
        assert_eq!(
            session.value().unwrap()[key].as_array().unwrap().last(),
            Some(&expected)
        );
    }
}

#[tokio::test]
async fn markdown_escapes_stay_out_of_normal_fields_and_list_previews() {
    let (temp, mut state, cookie, actor) = field_types_fixture().await;
    let root = temp.path().join("project");
    let schema_path = root.join(".editor/objects.schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&schema_path).unwrap()).unwrap();
    schema["anyOf"][0]["x-search"] = json!(["inline", "notes"]);
    std::fs::write(schema_path, schema.to_string()).unwrap();
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    let doc = state.editor.get("notes", "one").unwrap();
    let mut value = doc.value;
    let imported = r"Der neue Menoza. \[[→](https://example.test)\]";
    let readable = "Der neue Menoza. [[→](https://example.test)]";
    let plain = "He said \"hello\"\nC:\\Users\\name\\file.txt";
    value["notes"] = json!([imported]);
    value["inline"] = json!(imported);
    value["summary"] = json!(plain);
    state
        .editor
        .save("notes", "one", value.clone(), Some(&doc.revision))
        .unwrap();
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let (_, html) = request(
        state.clone(),
        "GET",
        &format!("/edit-sessions/{}", session.id),
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(html.contains(readable));
    assert!(!html.contains(imported));
    assert!(html.contains(&views::escape(plain)));
    // Rendering must not rewrite the source file or the in-memory draft.
    assert_eq!(state.editor.get("notes", "one").unwrap().value, value);
    assert_eq!(session.value().unwrap(), value);
    let (_, list) = request(
        state.clone(),
        "GET",
        "/collections/notes?columns=custom&column.notes=1&column.inline=1&q=%E2%86%92",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(list.contains("[<mark>→</mark>]"), "{list}");
    assert!(!list.contains(r"\["));
    assert!(!list.contains("https://example.test"));
    let inline = session
        .draft
        .fields
        .iter()
        .find(|f| f.key == "inline")
        .unwrap();
    let mut values = form(&session, &actor);
    values.insert(format!("value.{}", inline.id), readable.into());
    let (status, _) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/save", session.id),
        &cookie,
        &values,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let saved = state.editor.get("notes", "one").unwrap().value;
    // An unchanged readable control must not rewrite the stored Markdown.
    assert_eq!(saved["inline"], imported);
    assert_eq!(saved["summary"], plain);
    assert!(
        views::record(&state.editor, "notes", "one", &actor)
            .unwrap()
            .contains("[<a href=\"https://example.test\"")
    );
}

#[tokio::test]
async fn mapped_documents_share_drafts_controls_validation_and_conflict_guards() {
    let (temp, state, cookie, actor) = fixture_with_settings_schema(json!({
        "type":"object", "required":["title"], "properties":{
            "title":{"type":"string","minLength":1},
            "items":{"type":"array","items":{"type":"integer"}}
        }
    }))
    .await;
    std::fs::write(
        temp.path().join("project/settings.json"),
        r#"{"title":"Document","items":[1,2],"unknown":"keep"}"#,
    )
    .unwrap();
    let form_start = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("file".into(), "settings.json".into()),
    ]);
    let (status, html) = request(
        state.clone(),
        "POST",
        "/documents/editor-settings/edit-sessions",
        &cookie,
        &form_start,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("settings.json"));
    assert!(html.contains("</i>Add</button>"));
    assert!(!html.contains("name=\"record_id\""));
    let id = html
        .split("data-draft-id=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .get(id, &actor)
        .unwrap()
        .clone();
    let title = session
        .draft
        .fields
        .iter()
        .find(|f| f.key == "title")
        .unwrap()
        .id
        .clone();
    let mut submitted = form(&session, &actor);
    submitted.insert(format!("value.{title}"), String::new());
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{id}/save"),
        &cookie,
        &submitted,
        true,
    )
    .await;
    assert!(html.contains("issue(s) to resolve"));
    assert_eq!(state.editor.settings().unwrap().value["title"], "Document");
    session = state
        .sessions
        .lock()
        .unwrap()
        .get(id, &actor)
        .unwrap()
        .clone();
    submitted = form(&session, &actor);
    submitted.insert(format!("value.{title}"), "Updated".into());
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{id}/save"),
        &cookie,
        &submitted,
        true,
    )
    .await;
    assert!(html.contains("Saved to disk."));
    assert_eq!(state.editor.settings().unwrap().value["unknown"], "keep");
    assert_eq!(
        state.editor.settings().unwrap().value["items"],
        json!([1, 2])
    );
    assert_eq!(state.editor.settings().unwrap().value["title"], "Updated");
    session = state
        .sessions
        .lock()
        .unwrap()
        .get(id, &actor)
        .unwrap()
        .clone();
    std::fs::write(
        temp.path().join("project/settings.json"),
        r#"{"title":"External"}"#,
    )
    .unwrap();
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{id}/save"),
        &cookie,
        &form(&session, &actor),
        true,
    )
    .await;
    assert!(html.contains("changed on disk"));
    assert_eq!(state.editor.settings().unwrap().value["title"], "External");
}

#[tokio::test]
async fn child_list_overrides_are_local_and_used_by_preview_and_full_list() {
    let (temp, mut state, cookie, actor) = hierarchy_fixture().await;
    let root = temp.path().join("project");
    let mut ui: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".editor/ui.json")).unwrap()).unwrap();
    ui["collections"]["notes"]["list"] =
        json!({"columns":["name","count"],"defaultSort":{"field":"name","direction":"asc"}});
    ui["collections"]["groups"]["childLists"] = json!({
        "defaults":{"list":{"previewSize":1,"defaultSort":{"direction":"desc"}}},
        "fields":{"children":{"list":{"columns":["count"],"filters":["count"]}}}
    });
    std::fs::write(root.join(".editor/ui.json"), ui.to_string()).unwrap();
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    let resolved = state.editor.child_list_ui("groups", "children").unwrap();
    assert_eq!(
        resolved["list"]["defaultSort"],
        json!({"field":"name","direction":"desc"})
    );
    assert_eq!(
        state.editor.config().collection("notes").unwrap().ui["list"]["columns"],
        json!(["name", "count"])
    );
    let preview =
        hierarchy::sections(&state.editor, "groups", "g", &actor, &HashMap::new()).unwrap();
    assert!(preview.contains("sort=count"));
    assert!(!preview.contains("sort=name"));
    let (status, html) = request(state.clone(), "GET", "/collections/notes?parent_collection=groups&parent_id=g&parent_field=children&filter.count.min=1", &cookie, &HashMap::new(), false).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("data-label=\"count\""));
    assert!(!html.contains("data-label=\"name\""));
    // Reset the local list: inherited sort and columns disappear only for this occurrence.
    ui["collections"]["groups"]["childLists"]["fields"]["children"] =
        json!({"inherit":false,"list":{"columns":["_id"]}});
    std::fs::write(root.join(".editor/ui.json"), ui.to_string()).unwrap();
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    assert!(
        state.editor.child_list_ui("groups", "children").unwrap()["list"]["defaultSort"].is_null()
    );
    assert_eq!(
        state.editor.config().collection("notes").unwrap().ui["list"]["columns"],
        json!(["name", "count"])
    );
}

#[tokio::test]
async fn document_presentation_inherits_global_defaults_and_supports_raw_root() {
    let (temp, mut state, _, actor) = fixture_with_settings_schema(
        json!({"type":"object","properties":{"title":{"type":"string"}}}),
    )
    .await;
    let root = temp.path().join("project");
    let mut ui: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".editor/ui.json")).unwrap()).unwrap();
    ui["defaults"] = json!({"presentation":{"editor":"raw"}});
    ui["documents"] = json!({"editor-settings":{"presentation":{"inherit":false,"editor":"form"}}});
    std::fs::write(root.join(".editor/ui.json"), ui.to_string()).unwrap();
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    assert!(session.raw.is_none());
    assert!(!session.draft.root_value);
    ui["documents"] = json!({});
    std::fs::write(root.join(".editor/ui.json"), ui.to_string()).unwrap();
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    assert_eq!(session.raw.as_deref(), Some("{}"));
}

#[tokio::test]
async fn lists_and_object_views_offer_explicit_edit_buttons() {
    let (_temp, state, cookie, _) = fixture().await;
    let (_, list) = request(
        state.clone(),
        "GET",
        "/collections/notes",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(list.contains("href=\"/collections/notes/records/one/edit\""));
    assert!(list.contains("href=\"/collections/notes/records/one\""));
    let (_, preview) = request(
        state.clone(),
        "GET",
        "/collections/notes/records/one",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(preview.contains("href=\"/collections/notes/records/one/edit?node="));
    let (_, documents) = request(
        state.clone(),
        "GET",
        "/documents",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(documents.contains("/edit?file="));
    assert!(documents.contains("aria-label=\"Edit settings.json\""));
    let (_, document) = request(
        state.clone(),
        "GET",
        "/documents/editor-settings?file=settings.json",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(document.contains("/edit?file="));
}

#[tokio::test]
async fn boolean_filters_always_offer_true_false_and_unset() {
    let (temp, mut state, cookie, _) = fixture().await;
    let root = temp.path().join("project");
    let schema_path = root.join(".editor/objects.schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&schema_path).unwrap()).unwrap();
    schema["anyOf"][0]["properties"]["Verschollen"] = json!({"type":"boolean"});
    std::fs::write(schema_path, schema.to_string()).unwrap();
    let ui_path = root.join(".editor/ui.json");
    let mut ui: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&ui_path).unwrap()).unwrap();
    ui["collections"]["notes"]["list"] = json!({"filters":["Verschollen"]});
    std::fs::write(ui_path, ui.to_string()).unwrap();
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    // Even when no records have either boolean value, all three choices are available.
    let (_, html) = request(
        state.clone(),
        "GET",
        "/collections/notes",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    for option in ["true", "false", "__missing"] {
        assert!(html.contains(&format!("<option value=\"{option}\" >")));
    }
    for (id, value) in [("yes", true), ("no", false)] {
        state
            .editor
            .save(
                "notes",
                id,
                json!({"name":id,"count":1,"Verschollen":value}),
                None,
            )
            .unwrap();
    }
    for (option, id) in [("true", "yes"), ("false", "no"), ("__missing", "one")] {
        let page = state
            .editor
            .query_collection(
                "notes",
                &structured_editor_core::ListRequest {
                    filters: std::collections::BTreeMap::from([(
                        "Verschollen".into(),
                        option.into(),
                    )]),
                    limit: 50,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page.matched, 1);
        assert_eq!(page.rows[0].id, id);
    }
}

#[tokio::test]
async fn recursive_browsing_preserves_unsaved_siblings_and_saves_deep_edits() {
    let (temp, mut state, cookie, actor) = fixture().await;
    let root = temp.path().join("project");
    let schema = json!({"type":"object","properties":{"title":{"type":"string"},"branch":{"type":"object","properties":{"items":{"type":"array","items":{"type":"object","properties":{"deep":{"type":"object","properties":{"text":{"type":"string"}}}}}}}}}});
    std::fs::write(
        root.join(".editor/settings.schema.json"),
        schema.to_string(),
    )
    .unwrap();
    let original = json!({"title":"original","branch":{"items":[{"deep":{"text":"leaf"}}]}});
    std::fs::write(root.join("settings.json"), original.to_string()).unwrap();
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    let title = session.draft.at_path("/title").unwrap().id.clone();
    let deep = session
        .draft
        .at_path("/branch/items/0/deep")
        .unwrap()
        .id
        .clone();
    let text = session
        .draft
        .at_path("/branch/items/0/deep/text")
        .unwrap()
        .id
        .clone();
    let form = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), session.version.to_string()),
        (format!("present.{title}"), "1".into()),
        (format!("value.{title}"), "unsaved title".into()),
    ]);
    let (status, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/fields/{deep}/actions/browse", session.id),
        &cookie,
        &form,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains(&format!("name=\"value.{text}\"")));
    assert!(!html.contains(&format!("name=\"value.{title}\"")));
    session = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert_eq!(session.view_path, "/branch/items/0/deep");
    let form = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), session.version.to_string()),
        (format!("present.{text}"), "1".into()),
        (format!("value.{text}"), "edited leaf".into()),
    ]);
    let (status, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/save", session.id),
        &cookie,
        &form,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{html}");
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("settings.json")).unwrap()).unwrap();
    assert_eq!(saved["title"], "unsaved title");
    assert_eq!(
        saved.pointer("/branch/items/0/deep/text"),
        Some(&json!("edited leaf"))
    );
    let (_, html) = request(
        state,
        "GET",
        "/documents/editor-settings?file=settings.json&node=%2Fbranch%2Fitems%2F0%2Fdeep",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(html.contains("edited leaf"));
    assert!(!html.contains("Document tree"));
    assert!(!html.contains("Document root"));
    assert!(html.contains("data-navigation"));
    assert!(html.contains("aria-label=\"Record location\""));
    assert!(html.contains("node=%2Fbranch%2Fitems"));
}

#[tokio::test]
async fn recursive_endpoint_rendering_respects_raw_policy_and_hides_descendant_navigation() {
    let schema = json!({"type":"object","x-presentation":{"rawFallback":false},"properties":{"endpoint":{"type":"object","x-presentation":{"navigation":"stop"},"properties":{"notes":{"type":"array","items":{"type":"string","x-editor":"markdown"}},"blocked":{"type":"object"},"source":{"type":"object","x-presentation":{"editor":"raw"}}}}}});
    let (temp, state, _, actor) = fixture_with_settings_schema(schema).await;
    let value =
        json!({"endpoint":{"notes":["text"],"blocked":{"secret":"preserve"},"source":{"a":1}}});
    std::fs::write(temp.path().join("project/settings.json"), value.to_string()).unwrap();
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    session.view_path = "/endpoint".into();
    let source = &session.draft.at_path("/endpoint/source").unwrap().id;
    let blocked = &session.draft.at_path("/endpoint/blocked").unwrap().id;
    let html = views::edit(&state.editor, &session, &actor, &state.fields, &[], "").unwrap();
    assert!(html.contains(&format!("name=\"value.{source}\"")));
    assert!(!html.contains(&format!("name=\"value.{blocked}\"")));
    assert!(html.contains("automatic raw fallback is disabled"));
    let nav = crate::tree::edit_navigation(&session);
    assert!(!nav.contains(&format!("/fields/{source}/actions/browse")));
    assert!(!nav.contains(&format!("/fields/{blocked}/actions/browse")));
    assert_eq!(session.value().unwrap(), value);
}

#[tokio::test]
async fn invalid_nested_input_survives_navigation_and_undo_and_feedback_targets_visible_fields() {
    let schema = json!({"type":"object","properties":{"branch":{"type":"object","properties":{"a/b~c":{"type":"integer"}}},"sibling":{"type":"string"}}});
    let (temp, state, cookie, actor) = fixture_with_settings_schema(schema).await;
    let file = temp.path().join("project/settings.json");
    let value = json!({"branch":{"a/b~c":7},"sibling":"unchanged"});
    std::fs::write(&file, value.to_string()).unwrap();
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    let branch = session.draft.at_path("/branch").unwrap().id.clone();
    let number = session.draft.at_path("/branch/a~1b~0c").unwrap().id.clone();
    let sibling = session.draft.at_path("/sibling").unwrap().id.clone();
    for (action, raw) in [
        (format!("fields/{branch}/actions/browse"), None),
        ("browse-root".into(), Some("invalid integer")),
        (format!("fields/{branch}/actions/browse"), None),
    ] {
        let mut form = HashMap::from([
            ("csrf".into(), actor.csrf.clone()),
            ("version".into(), session.version.to_string()),
        ]);
        if let Some(raw) = raw {
            form.insert(format!("present.{number}"), "1".into());
            form.insert(format!("value.{number}"), raw.into());
        }
        let (status, html) = request(
            state.clone(),
            "POST",
            &format!("/edit-sessions/{}/{action}", session.id),
            &cookie,
            &form,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{html}");
        session = state
            .sessions
            .lock()
            .unwrap()
            .get(&session.id, &actor)
            .unwrap()
            .clone();
    }
    assert_eq!(
        session.draft.at_path("/branch/a~1b~0c").unwrap().raw,
        "invalid integer"
    );
    let issues = session.value().unwrap_err();
    assert_eq!(issues[0].path, "/branch/a~1b~0c");
    let fragments = views::validation::fragments(&session, &issues);
    assert!(
        fragments
            .iter()
            .any(|f| f.contains(&format!("id=\"errors-{number}\"")) && f.contains("valid integer"))
    );
    assert!(
        !fragments
            .iter()
            .any(|f| f.contains(&format!("id=\"errors-{sibling}\"")))
    );
    let form = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), session.version.to_string()),
    ]);
    let (status, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/undo", session.id),
        &cookie,
        &form,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{html}");
    let restored = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert_eq!(restored.value().unwrap(), value);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(file).unwrap()).unwrap(),
        value
    );
}

#[tokio::test]
async fn linked_list_actions_offer_unlink_and_reviewed_delete_with_independent_permissions() {
    let (temp, mut state, cookie, actor) = hierarchy_fixture().await;
    for url in [
        "/collections/groups/records/g",
        "/collections/notes?parent_collection=groups&parent_id=g&parent_field=children",
    ] {
        let (status, html) =
            request(state.clone(), "GET", url, &cookie, &HashMap::new(), false).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains("Unlink from this list"), "{url}");
        assert!(
            html.contains("href=\"/collections/notes/records/one/delete\""),
            "{url}"
        );
    }
    let before = state.editor.get("notes", "one").unwrap();
    let (_, html) = request(
        state.clone(),
        "GET",
        "/collections/notes/records/one/delete",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(html.contains("class=\"danger\" disabled"));
    assert_eq!(
        state.editor.get("notes", "one").unwrap().revision,
        before.revision
    );
    let root = temp.path().join("project");
    upgrade_action_fixture(&root);
    let path = root.join(".editor/notes.settings.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["operations"] = json!(["read", "update"]);
    std::fs::write(&path, config.to_string()).unwrap();
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    let menu = crate::hierarchy::record_actions(
        &state.editor,
        &actor,
        "notes",
        "one",
        "First",
        Some(("groups", "g", "children", "revision")),
    )
    .unwrap();
    assert!(menu.contains("Unlink from this list"));
    assert!(!menu.contains("/records/one/delete"));
    config["operations"] = json!(["read", "delete"]);
    let group_path = root.join(".editor/groups.settings.json");
    let mut group: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&group_path).unwrap()).unwrap();
    group["operations"] = json!(["read"]);
    std::fs::write(group_path, group.to_string()).unwrap();
    std::fs::write(path, config.to_string()).unwrap();
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    let menu = crate::hierarchy::record_actions(
        &state.editor,
        &actor,
        "notes",
        "one",
        "First",
        Some(("groups", "g", "children", "revision")),
    )
    .unwrap();
    assert!(!menu.contains("Unlink from this list"));
    assert!(menu.contains("/records/one/delete"));
    let mut viewer = actor.clone();
    viewer.role = "viewer".into();
    assert!(
        crate::hierarchy::record_actions(&state.editor, &viewer, "notes", "one", "First", None)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn array_actions_distinguish_embedded_deletion_from_id_unlinking() {
    let schema = json!({"type":"object","properties":{"embedded":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"}}}},"linked":{"type":"array","items":{"type":"string","x-relation":{"collection":"notes"}}}}});
    let (temp, mut state, _, actor) = fixture_with_settings_schema(schema).await;
    let root = temp.path().join("project");
    upgrade_action_fixture(&root);
    let ui_path = root.join(".editor/ui.json");
    let mut ui: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&ui_path).unwrap()).unwrap();
    ui["collections"]["notes"]["singularLabel"] = json!("Note");
    std::fs::write(ui_path, ui.to_string()).unwrap();
    std::fs::write(
        root.join("settings.json"),
        json!({"embedded":[{"name":"nested"}],"linked":["one"]}).to_string(),
    )
    .unwrap();
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    session.view_path = "/embedded".into();
    let html = views::edit(&state.editor, &session, &actor, &state.fields, &[], "").unwrap();
    assert!(html.contains("Delete row 1"));
    assert!(html.contains("Delete item and its nested contents when saved"));
    assert!(!html.contains("Unlink item 1"));
    session.view_path.clear();
    let html = views::edit(&state.editor, &session, &actor, &state.fields, &[], "").unwrap();
    assert!(html.contains("Remove First"));
    assert!(!html.contains("href=\"/collections/notes/records/one/delete\""));
    assert!(!html.contains("Enter ID manually"));
    assert!(html.contains("leaves it unset when saved, rather than empty"));
}

fn upgrade_action_fixture(root: &std::path::Path) {
    let repository = root.join(".editor/repository.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&repository).unwrap()).unwrap();
    let schemas: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".editor/objects.schema.json")).unwrap())
            .unwrap();
    for (name, storage) in config["collections"].as_object_mut().unwrap() {
        let mut schema = schemas["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["x-collection"] == *name)
            .unwrap()
            .clone();
        schema.as_object_mut().unwrap().remove("x-collection");
        let schema_path = format!(".editor/{name}.schema.json");
        std::fs::write(root.join(&schema_path), schema.to_string()).unwrap();
        let mut settings = storage.clone();
        settings["type"] = json!("data");
        settings["schema"] = json!(schema_path);
        let ui_path = format!(".editor/{name}.ui.json");
        std::fs::write(root.join(&ui_path), "{}").unwrap();
        settings["ui"] = json!(ui_path);
        let path = format!(".editor/{name}.settings.json");
        std::fs::write(root.join(&path), settings.to_string()).unwrap();
        *storage = json!(path);
    }
    config["version"] = json!(2);
    config.as_object_mut().unwrap().remove("objects");
    std::fs::write(repository, config.to_string()).unwrap();
}

#[tokio::test]
async fn mapped_reference_validation_blocks_raw_saves_and_exposes_document_in_delete_review() {
    let schema = json!({"type":"object","properties":{"links":{"type":"array","items":{"type":"string","x-relation":{"collection":"notes"}}}}});
    let (temp, state, cookie, actor) = fixture_with_settings_schema(schema).await;
    let file = temp.path().join("project/settings.json");
    let original = std::fs::read(&file).unwrap();
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    session.raw = Some("{}".into());
    state.sessions.lock().unwrap().insert(session.clone());
    let values = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), session.version.to_string()),
        ("raw".into(), json!({"links":["missing"]}).to_string()),
    ]);
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/save", session.id),
        &cookie,
        &values,
        false,
    )
    .await;
    assert!(html.contains("/links/0") && html.contains("unresolved notes reference"));
    assert_eq!(std::fs::read(&file).unwrap(), original);
    let doc = state
        .editor
        .json_document("editor-settings", "settings.json")
        .unwrap();
    state
        .editor
        .save_json_document(
            "editor-settings",
            "settings.json",
            json!({"links":["one"]}),
            &doc.revision,
        )
        .unwrap();
    let (_, html) = request(
        state,
        "GET",
        "/collections/notes/records/one/delete",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(
        html.contains("/documents/editor%2Dsettings?file=settings%2Ejson")
            || html.contains("/documents/editor-settings?file=settings%2Ejson"),
        "{html}"
    );
    assert!(html.contains("/links/0"));
    assert!(html.contains("class=\"danger\" disabled"));
}

#[tokio::test]
async fn system_metadata_is_readable_in_root_and_nested_document_views_and_edits() {
    let schema = json!({"type":"object","properties":{"branch":{"type":"object","properties":{"title":{"type":"string"}}}}});
    let (temp, state, cookie, actor) = fixture_with_settings_schema(schema).await;
    let revision = "abcdef0123456789abcdef0123456789abcdef01";
    let metadata = json!({"schema_revision":revision,"manual_order":0,"extra":{"enabled":true,"note":"<script>bad</script>"}});
    let data = json!({"_editor":metadata,"branch":{"title":"Branch","_editor":metadata}});
    std::fs::write(temp.path().join("project/settings.json"), data.to_string()).unwrap();
    for url in [
        "/documents/editor-settings?file=settings.json",
        "/documents/editor-settings?file=settings.json&node=%2Fbranch",
    ] {
        let (status, html) =
            request(state.clone(), "GET", url, &cookie, &HashMap::new(), false).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!html.contains("Editor metadata"));
        assert!(html.contains("record-metadata"));
        assert!(html.contains("Schema revision"));
        assert!(html.contains("\">abcdef01</code>"), "{url}: {html}");
        assert!(html.contains("Manual order"));
        assert!(html.contains("<span class=\"mono\">0</span>"));
        assert!(html.contains("&lt;script&gt;bad&lt;/script&gt;"));
        assert!(!html.contains("<pre"));
        assert!(!html.contains("&quot;schema_revision&quot;"));
    }
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    let root = session.draft.at_path("/_editor").unwrap().id.clone();
    let nested = session.draft.at_path("/branch/_editor").unwrap().id.clone();
    session.view_path = "/branch".into();
    let html = views::edit(&state.editor, &session, &actor, &state.fields, &[], "").unwrap();
    for id in [&root, &nested] {
        assert!(html.contains(&format!("name=\"manual_order.{id}\"")));
        assert!(!html.contains(&format!("name=\"value.{id}\"")));
    }
    assert!(html.contains("\">abcdef01</code>"));
    let values = HashMap::from([
        ("version".into(), session.version.to_string()),
        (format!("present.{nested}"), "1".into()),
        (format!("manual_order.{nested}"), "7".into()),
    ]);
    session.apply(&state.editor, &values).unwrap();
    let result = session.value().unwrap();
    assert_eq!(result["_editor"], metadata);
    assert_eq!(result["branch"]["_editor"]["schema_revision"], revision);
    assert_eq!(result["branch"]["_editor"]["manual_order"], 7);
    assert_eq!(result["branch"]["_editor"]["extra"], metadata["extra"]);
}

#[tokio::test]
async fn record_metadata_preview_uses_system_fields_instead_of_raw_json() {
    let (_temp, state, _, actor) = fixture().await;
    let doc = state.editor.get("notes", "one").unwrap();
    let mut data = doc.value;
    data["_editor"] =
        json!({"schema_revision":"0123456789abcdef0123456789abcdef01234567","manual_order":12});
    // Render the same shared preview used by record and document routes.
    let draft = structured_editor_core::draft::Draft::new(&json!({"type":"object"}), &data);
    let html = crate::tree::preview(
        &state.editor,
        &draft,
        &data,
        "/collections/notes/records/one",
        "",
    );
    assert!(html.contains("Schema revision") && html.contains("\">01234567</code>"));
    assert!(html.contains("Manual order") && html.contains("<span class=\"mono\">12</span>"));
    assert!(!html.contains("&quot;schema_revision&quot;"));
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    session.draft = draft;
    let html = views::edit(&state.editor, &session, &actor, &state.fields, &[], "").unwrap();
    assert_eq!(html.matches("<code>01234567</code>").count(), 1);
    assert!(html.contains("Manual order"));
}

#[tokio::test]
async fn unrelated_edit_sessions_progress_while_another_session_is_locked() {
    let (_temp, state, cookie, actor) = fixture().await;
    let first = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let second = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let gate = state
        .sessions
        .lock()
        .unwrap()
        .gate(&first.id, &actor)
        .unwrap();
    let held = gate.lock().unwrap();
    let form = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), "0".into()),
    ]);
    let blocked = {
        let state = state.clone();
        let cookie = cookie.clone();
        let form = form.clone();
        let id = first.id.clone();
        tokio::spawn(async move {
            request(
                state,
                "POST",
                &format!("/edit-sessions/{id}/validate"),
                &cookie,
                &form,
                false,
            )
            .await
        })
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        request(
            state.clone(),
            "POST",
            &format!("/edit-sessions/{}/validate", second.id),
            &cookie,
            &form,
            false,
        ),
    )
    .await;
    let still_blocked = !blocked.is_finished();
    drop(held); // Always release before asserting, including timeout failures.
    assert_eq!(result.unwrap().0, StatusCode::OK);
    assert!(still_blocked);
    assert_eq!(blocked.await.unwrap().0, StatusCode::OK);
    // Two concurrent commands with the same version cannot both replace the draft.
    let form = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), "1".into()),
    ]);
    let url = format!("/edit-sessions/{}/validate", first.id);
    let (a, b) = tokio::join!(
        request(state.clone(), "POST", &url, &cookie, &form, false),
        request(state.clone(), "POST", &url, &cookie, &form, false)
    );
    assert_eq!(
        [a.0, b.0].iter().filter(|s| **s == StatusCode::OK).count(),
        1
    );
    assert_eq!(
        state
            .sessions
            .lock()
            .unwrap()
            .get(&first.id, &actor)
            .unwrap()
            .version,
        2
    );
    let session = state
        .sessions
        .lock()
        .unwrap()
        .get(&first.id, &actor)
        .unwrap()
        .clone();
    state.sessions.lock().unwrap().logout(&actor);
    assert!(
        state
            .sessions
            .lock()
            .unwrap()
            .update(&actor, session)
            .is_err()
    );
}

fn draft_id(html: &str) -> String {
    html.split("data-draft-id=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .into()
}
#[tokio::test]
async fn stable_edit_urls_resume_private_drafts_and_restore_unsubmitted_input() {
    let (_temp, state, cookie, actor) = fixture().await;
    let (_, entry) = request(
        state.clone(),
        "GET",
        "/collections/notes/records/one/edit",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(entry.contains("data-draft-entry"));
    assert!(!entry.contains("data-draft-id"));
    let start = HashMap::from([("csrf".into(), actor.csrf.clone())]);
    let (_, html) = request(
        state.clone(),
        "POST",
        "/collections/notes/records/one/edit-sessions",
        &cookie,
        &start,
        true,
    )
    .await;
    let id = draft_id(&html);
    assert!(html.contains("data-url=\"/collections/notes/records/one/edit\""));
    let session = state
        .sessions
        .lock()
        .unwrap()
        .get(&id, &actor)
        .unwrap()
        .clone();
    let mut input = form(&session, &actor);
    let name = session
        .draft
        .fields
        .iter()
        .find(|f| f.key == "name")
        .unwrap();
    input.insert(format!("value.{}", name.id), "Recovered typing".into());
    let mut resume = start.clone();
    resume.insert("draft".into(), id.clone());
    resume.insert("restore".into(), serde_json::to_string(&input).unwrap());
    let (_, html) = request(
        state.clone(),
        "POST",
        "/collections/notes/records/one/edit-sessions",
        &cookie,
        &resume,
        true,
    )
    .await;
    assert_eq!(draft_id(&html), id);
    assert!(html.contains("Recovered typing"));
    assert_eq!(
        state.editor.get("notes", "one").unwrap().value["name"],
        "First"
    );
    assert!(
        state
            .sessions
            .lock()
            .unwrap()
            .get(&id, &actor)
            .unwrap()
            .dirty()
    );
    // A fresh tab gets an independent draft; it does not attach to a record-wide session.
    let (_, html) = request(
        state.clone(),
        "POST",
        "/collections/notes/records/one/edit-sessions",
        &cookie,
        &start,
        true,
    )
    .await;
    assert_ne!(draft_id(&html), id);
    let mut stranger = actor.clone();
    stranger.session = "other-login".into();
    assert!(
        crate::draft_lifecycle::resume(
            &state,
            &stranger,
            &resume,
            "/collections/notes/records/one/edit"
        )
        .is_err()
    );
}
#[tokio::test]
async fn close_discard_and_save_close_preserve_data_and_release_drafts() {
    let (_temp, state, cookie, actor) = fixture().await;
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let id = session.id.clone();
    let mut input = form(&session, &actor);
    let name = session
        .draft
        .fields
        .iter()
        .find(|f| f.key == "name")
        .unwrap()
        .id
        .clone();
    input.insert(format!("value.{name}"), "Unsubmitted edit".into());
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{id}/close"),
        &cookie,
        &input,
        true,
    )
    .await;
    assert!(html.contains("Discard changes?"));
    assert!(html.contains("Discard changes") && html.contains("Keep editing"));
    assert_eq!(
        state.editor.get("notes", "one").unwrap().value["name"],
        "First"
    );
    session = state
        .sessions
        .lock()
        .unwrap()
        .get(&id, &actor)
        .unwrap()
        .clone();
    request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{id}/keep-editing"),
        &cookie,
        &form(&session, &actor),
        true,
    )
    .await;
    session = state
        .sessions
        .lock()
        .unwrap()
        .get(&id, &actor)
        .unwrap()
        .clone();
    assert!(!session.close_requested && session.dirty());
    let mut doc = state.editor.get("notes", "one").unwrap();
    doc.value["name"] = json!("External update");
    state
        .editor
        .save("notes", "one", doc.value, Some(&doc.revision))
        .unwrap();
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{id}/discard"),
        &cookie,
        &form(&session, &actor),
        true,
    )
    .await;
    assert!(html.contains("data-closed-draft"));
    assert!(html.contains("data-navigate-to=\"/collections/notes/records/one\""));
    assert!(state.sessions.lock().unwrap().get(&id, &actor).is_err());
    assert_eq!(
        state.editor.get("notes", "one").unwrap().value["name"],
        "External update"
    );
    session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let id = session.id.clone();
    let mut input = form(&session, &actor);
    let count = session
        .draft
        .fields
        .iter()
        .find(|f| f.key == "count")
        .unwrap()
        .id
        .clone();
    input.insert(format!("value.{count}"), "invalid".into());
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{id}/save-close"),
        &cookie,
        &input,
        true,
    )
    .await;
    assert!(html.contains("record-form") && !html.contains("data-closed-draft"));
    session = state
        .sessions
        .lock()
        .unwrap()
        .get(&id, &actor)
        .unwrap()
        .clone();
    let mut input = form(&session, &actor);
    input.insert(format!("value.{count}"), "7".into());
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{id}/save-close"),
        &cookie,
        &input,
        true,
    )
    .await;
    assert!(
        html.contains("data-closed-draft")
            && html.contains("data-navigate-to=\"/collections/notes/records/one\"")
    );
    assert!(state.sessions.lock().unwrap().get(&id, &actor).is_err());
    assert_eq!(state.editor.get("notes", "one").unwrap().value["count"], 7);
    for action in ["close", "discard-close"] {
        let session = state
            .sessions
            .lock()
            .unwrap()
            .start(&state.editor, &actor, "notes", Some("one"))
            .unwrap();
        let (_, html) = request(
            state.clone(),
            "POST",
            &format!("/edit-sessions/{}/{action}", session.id),
            &cookie,
            &form(&session, &actor),
            true,
        )
        .await;
        assert!(html.contains("data-closed-draft"));
        assert!(
            state
                .sessions
                .lock()
                .unwrap()
                .get(&session.id, &actor)
                .is_err()
        );
    }
}
#[tokio::test]
async fn expired_and_closed_drafts_offer_explicit_recovery() {
    let (_temp, state, cookie, actor) = fixture().await;
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let mut input = form(&session, &actor);
    input.insert("edit_url".into(), session.edit_url());
    session.touched = auth::now() - 7201;
    state.sessions.lock().unwrap().insert(session.clone());
    state.sessions.lock().unwrap().expire();
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/save", session.id),
        &cookie,
        &input,
        true,
    )
    .await;
    assert!(html.contains("Draft expired") && html.contains("Reopen editor"));
    assert!(!html.contains("record-form"));
    let start = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("draft".into(), session.id),
    ]);
    let (_, html) = request(
        state.clone(),
        "POST",
        "/collections/notes/records/one/edit-sessions",
        &cookie,
        &start,
        true,
    )
    .await;
    assert!(
        html.contains("data-expired=\"true\"")
            && html.contains("unsaved server-side changes are no longer available")
    );
    assert!(!html.contains("data-draft-id"));
}
#[tokio::test]
async fn mapped_documents_use_stable_edit_urls_and_save_keeps_draft_open() {
    let (_temp, state, cookie, actor) = fixture().await;
    let (_, html) = request(
        state.clone(),
        "GET",
        "/documents/editor-settings/edit?file=settings.json",
        &cookie,
        &HashMap::new(),
        true,
    )
    .await;
    assert!(html.contains("data-draft-entry"));
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/save", session.id),
        &cookie,
        &form(&session, &actor),
        true,
    )
    .await;
    assert!(html.contains("/documents/editor%2Dsettings/edit?file=settings%2Ejson"));
    assert_eq!(draft_id(&html), session.id);
    let current = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert!(!current.dirty());
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/close", session.id),
        &cookie,
        &form(&current, &actor),
        true,
    )
    .await;
    assert!(
        html.contains("data-navigate-to=\"/documents/editor%2Dsettings?file=settings%2Ejson\"")
    );
}

#[path = "concurrency_tests.rs"]
mod concurrency;

#[tokio::test]
async fn link_picker_browses_media_and_uploads_without_replacing_the_draft() {
    let (_temp, state, cookie, actor) = fixture().await;
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let field = session
        .draft
        .fields
        .iter()
        .find(|field| field.key == "body")
        .unwrap()
        .id
        .clone();
    let mut input = form(&session, &actor);
    crate::pickers::action(&state.editor, &mut session, &field, "links", &input).unwrap();
    input.insert("picker_collection".into(), "media".into());
    input.insert("link_label".into(), "My scan".into());
    input.insert("link_image".into(), "on".into());
    crate::pickers::action(&state.editor, &mut session, &field, "search-picker", &input).unwrap();
    state
        .editor
        .for_media("media")
        .unwrap()
        .media_mkdir("scans")
        .unwrap();
    input.insert("choice".into(), "scans".into());
    crate::pickers::action(&state.editor, &mut session, &field, "folder", &input).unwrap();
    let html = crate::pickers::render(&state.editor, &session, &actor).unwrap();
    assert!(
        html.contains("optgroup label=\"Records\"")
            && html.contains("optgroup label=\"Media files\"")
    );
    assert!(html.contains("/media/media/upload"));
    assert!(html.contains("value=\"My scan\""));
    assert!(html.contains("name=\"path\" value=\"scans\""));
    let baseline = session.value().unwrap();
    state
        .sessions
        .lock()
        .unwrap()
        .update(&actor, session.clone())
        .unwrap();

    async fn upload(
        state: Arc<AppState>,
        cookie: &str,
        csrf: &str,
        session: &sessions::EditSession,
        path: &str,
        version: u64,
        name: &str,
    ) -> (StatusCode, String) {
        let boundary = "picker-upload-test";
        let mut body = String::new();
        for (key, value) in [
            ("csrf", csrf.to_owned()),
            ("picker_session", session.id.clone()),
            ("picker_version", version.to_string()),
            ("path", path.into()),
        ] {
            body += &format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{value}\r\n"
            );
        }
        body += &format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: text/plain\r\n\r\nScan content\r\n--{boundary}--\r\n"
        );
        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/media/media/upload")
                    .header("cookie", cookie)
                    .header("datastar-request", "true")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response
            .headers()
            .get("x-editor-error-status")
            .and_then(|value| StatusCode::from_bytes(value.as_bytes()).ok())
            .unwrap_or(response.status());
        let body = String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        (status, body)
    }
    for (csrf, path, version, expected) in [
        ("wrong", "scans", session.version, StatusCode::FORBIDDEN),
        (
            actor.csrf.as_str(),
            "",
            session.version,
            StatusCode::CONFLICT,
        ),
        (
            actor.csrf.as_str(),
            "scans",
            session.version + 1,
            StatusCode::CONFLICT,
        ),
    ] {
        assert_eq!(
            upload(
                state.clone(),
                &cookie,
                csrf,
                &session,
                path,
                version,
                "rejected.txt"
            )
            .await
            .0,
            expected
        );
        assert!(state.editor.media_read("scans/rejected.txt").is_err());
    }
    let (status, html) = upload(
        state.clone(),
        &cookie,
        &actor.csrf,
        &session,
        "scans",
        session.version,
        "scan.txt",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Insert uploaded file") && html.contains("value=\"scans/scan.txt\""));
    assert!(!html.contains("id=\"workspace\""));
    assert_eq!(
        state.editor.media_read("scans/scan.txt").unwrap(),
        b"Scan content"
    );
    assert_eq!(
        state
            .sessions
            .lock()
            .unwrap()
            .get(&session.id, &actor)
            .unwrap()
            .value()
            .unwrap(),
        baseline
    );
    assert_ne!(
        upload(
            state.clone(),
            &cookie,
            &actor.csrf,
            &session,
            "scans",
            session.version,
            "scan.txt"
        )
        .await
        .0,
        StatusCode::OK
    );
    {
        let mut registry = state.sessions.lock().unwrap();
        registry
            .get(&session.id, &actor)
            .unwrap()
            .picker
            .as_mut()
            .unwrap()
            .collection = "notes".into();
    }
    assert_eq!(
        upload(
            state.clone(),
            &cookie,
            &actor.csrf,
            &session,
            "scans",
            session.version,
            "wrong-collection.txt"
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert!(
        state
            .editor
            .media_read("scans/wrong-collection.txt")
            .is_err()
    );
    input.insert("choice".into(), "scans/scan.txt".into());
    crate::pickers::action(&state.editor, &mut session, &field, "select", &input).unwrap();
    assert!(
        session.value().unwrap()["body"]
            .as_str()
            .unwrap()
            .contains("![My scan](/media/media/file?path=scans%2Fscan%2Etxt)")
    );
    assert_eq!(
        state.editor.get("notes", "one").unwrap().value["body"],
        "**Hello**"
    );
}

#[tokio::test]
async fn exit_uses_actual_changes_and_confirms_only_once() {
    let (_temp, state, cookie, actor) = fixture().await;
    let original = state.editor.get("notes", "one").unwrap();
    let mut value = original.value.clone();
    value["body"] = json!(r"A \[caption\]");
    state
        .editor
        .save("notes", "one", value, Some(&original.revision))
        .unwrap();
    for (changed, confirm) in [(false, true), (true, true), (true, false)] {
        let mut session = state
            .sessions
            .lock()
            .unwrap()
            .start(&state.editor, &actor, "notes", Some("one"))
            .unwrap();
        let body = session
            .draft
            .fields
            .iter()
            .find(|field| field.key == "body")
            .unwrap()
            .id
            .clone();
        let mut input = form(&session, &actor);
        input.insert(
            format!("value.{body}"),
            if changed { "Edited" } else { "A [caption]" }.into(),
        );
        input.insert("confirm_discard".into(), confirm.to_string());
        let (_, html) = request(
            state.clone(),
            "POST",
            &format!("/edit-sessions/{}/close", session.id),
            &cookie,
            &input,
            true,
        )
        .await;
        if changed && confirm {
            assert_eq!(html.matches("id=\"draft-close-dialog\"").count(), 1);
            assert!(!html.contains("id=\"discard-dialog\""));
            session = state
                .sessions
                .lock()
                .unwrap()
                .get(&session.id, &actor)
                .unwrap()
                .clone();
            // Reverting before the next exit must also close without a dialog.
            let mut reverted = form(&session, &actor);
            reverted.insert(format!("value.{body}"), "A [caption]".into());
            let (_, html) = request(
                state.clone(),
                "POST",
                &format!("/edit-sessions/{}/close", session.id),
                &cookie,
                &reverted,
                true,
            )
            .await;
            assert!(html.contains("data-closed-draft"));
            assert!(!html.contains("<dialog"));
        } else {
            assert!(html.contains("data-closed-draft"));
            assert!(!html.contains("<dialog"));
        }
        assert_eq!(
            state.editor.get("notes", "one").unwrap().value["body"],
            r"A \[caption\]"
        );
    }
}

#[tokio::test]
async fn regex_search_renders_toggle_errors_and_library_highlights() {
    let (_temp, state, cookie, _) = fixture().await;
    let (_, html) = request(
        state.clone(),
        "GET",
        "/collections/notes?q=%5Ef.rst%24&regex=1",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(html.contains("aria-pressed=\"true\""));
    assert!(html.contains("<mark>First</mark>"));
    assert!(html.contains("regex=1"));
    assert_eq!(html.matches("aria-label=\"Actions for First\"").count(), 1);
    assert!(html.contains("</strong> match "));
    let (_, literal) = request(
        state.clone(),
        "GET",
        "/collections/notes?q=%5Ef.rst%24",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(!literal.contains("<mark>First</mark>"));
    for pattern in ["%5B", "%28%3F%3Da%29", "%28a%29%5C1"] {
        let (status, invalid) = request(
            state.clone(),
            "GET",
            &format!("/collections/notes?q={pattern}&regex=1"),
            &cookie,
            &HashMap::new(),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(invalid.contains("aria-invalid=\"true\""));
        assert!(invalid.contains("search-pattern-error"));
        assert!(invalid.contains("data-regex-search"));
        assert!(!invalid.contains("No matching records"));
    }
}

#[tokio::test]
async fn literal_case_matching_survives_navigation_and_show_all_resets_it() {
    let (temp, state, cookie, _) = fixture().await;
    state
        .editor
        .save("notes", "two", json!({"name":"first","count":2}), None)
        .unwrap();
    state
        .editor
        .save(
            "notes",
            "three",
            json!({"name":"First again","count":3}),
            None,
        )
        .unwrap();
    for (options, has_lowercase) in [
        ("", true),
        ("&case_sensitive=1", false),
        ("&case_sensitive=0", true),
    ] {
        let (status, html) = request(
            state.clone(),
            "GET",
            &format!("/collections/notes?q=First{options}"),
            &cookie,
            &HashMap::new(),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(html.contains("<mark>first</mark>"), has_lowercase);
        assert!(html.contains("<mark>First</mark>"));
        assert!(html.contains(&format!(
            "data-case-toggle aria-label=\"Match case\" aria-pressed=\"{}\"",
            !has_lowercase
        )));
        let search_form = html
            .split("data-regex-search")
            .nth(1)
            .unwrap()
            .split("</form>")
            .next()
            .unwrap();
        assert_eq!(search_form.matches("name=\"case_sensitive\"").count(), 1);
        assert_eq!(search_form.matches("name=\"regex\"").count(), 1);
    }
    // Small pages make preservation in sorting/paging links and column/filter
    // forms observable without constructing a large collection.
    std::fs::write(
        temp.path().join("project/settings.json"),
        r#"{"pageSize":1}"#,
    )
    .unwrap();
    state.editor.refresh_read_cache();
    let (_, html) = request(
        state.clone(),
        "GET",
        "/collections/notes?q=First&case_sensitive=1&sort=_label",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    let links: Vec<_> = html
        .split("href=\"")
        .skip(1)
        .map(|s| {
            percent_encoding::percent_decode_str(s.split('"').next().unwrap())
                .decode_utf8_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        links.iter().any(|link| link.contains("offset=1")
            && link.contains("case_sensitive=1")
            && link.contains("q=First")),
        "{links:?}"
    );
    assert!(
        links
            .iter()
            .any(|link| link.contains("direction=desc") && link.contains("case_sensitive=1"))
    );
    for form in html
        .split("<form")
        .skip(1)
        .map(|s| s.split("</form>").next().unwrap())
    {
        if form.contains("Visible columns") || form.contains("Apply filters") {
            assert!(form.contains("name=\"case_sensitive\" value=\"1\""));
        }
    }
    let show_all = html
        .split(">Show All</a>")
        .next()
        .unwrap()
        .rsplit("href=\"")
        .next()
        .unwrap()
        .trim_end_matches('"');
    assert!(!show_all.contains("case%5Fsensitive"));
    assert!(!show_all.contains("regex="));
    assert!(!show_all.contains("q="));
    assert!(show_all.contains("sort=%5Flabel"));
    let (status, partial) = request(
        state,
        "GET",
        "/collections/notes?q=First&case_sensitive=1&sort=_label&offset=1",
        &cookie,
        &HashMap::new(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(partial.contains("<mark>First</mark> again"));
    assert!(partial.contains("data-case-toggle aria-label=\"Match case\" aria-pressed=\"true\""));
}

#[tokio::test]
async fn every_embedded_table_has_independent_column_choices() {
    let (temp, mut state, cookie, _) = hierarchy_fixture().await;
    let root = temp.path().join("project");
    let schema_path = root.join(".editor/objects.schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&schema_path).unwrap()).unwrap();
    for collection in schema["anyOf"].as_array_mut().unwrap() {
        if collection["x-collection"] == "groups" {
            collection["properties"]["related"] = collection["properties"]["children"].clone();
            collection["properties"]["related"]["title"] = json!("Related notes");
        }
        if collection["x-collection"] == "notes" {
            collection["properties"]["secret"] = json!({"type":"string","x-editor":"hidden"});
        }
    }
    std::fs::write(&schema_path, schema.to_string()).unwrap();
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    let original = state.editor.get("groups", "g").unwrap();
    state
        .editor
        .save(
            "groups",
            "g",
            json!({"name":"Group","children":["one"],"related":["one"]}),
            Some(&original.revision),
        )
        .unwrap();
    let revision = state.editor.get("groups", "g").unwrap().revision;
    let config = std::fs::read(root.join(".editor/ui.json")).unwrap();
    let base = "/collections/groups/records/g";
    let (_, initial) = request(state.clone(), "GET", base, &cookie, &HashMap::new(), false).await;
    assert_eq!(initial.matches("<table ").count(), 2);
    assert_eq!(
        initial
            .matches("class=\"action-menu column-picker\"")
            .count(),
        2
    );
    assert!(initial.contains("id=\"columns-children-children\""));
    assert!(initial.contains("id=\"columns-children-related\""));
    let section = |html: &str, field: &str| -> String {
        html.split(&format!("id=\"children-{field}\""))
            .nth(1)
            .unwrap()
            .split("</section>")
            .next()
            .unwrap()
            .to_owned()
    };
    let url = format!(
        "{base}?child.children.columns=custom&child.children.column.count=1&child.related.columns=custom&child.related.column._id=1"
    );
    for partial in [false, true] {
        let (status, html) = request(
            state.clone(),
            "GET",
            &url,
            &cookie,
            &HashMap::new(),
            partial,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!html.contains("name=\"datastar\""));
        assert!(!html.contains("&amp;datastar="));
        let children = section(&html, "children");
        let related = section(&html, "related");
        assert!(children.contains("data-label=\"count\""));
        assert!(!children.contains("data-label=\"Identifier\""));
        assert!(related.contains("data-label=\"Identifier\""));
        assert!(!related.contains("data-label=\"count\""));
        // Updating either chooser retains the other table's selection.
        assert!(children.contains("type=\"hidden\" name=\"child.related.column._id\" value=\"1\""));
        assert!(
            related.contains("type=\"hidden\" name=\"child.children.column.count\" value=\"1\"")
        );
        assert!(html.contains(
            "data-url=\"/collections/groups/records/g?child%2Echildren%2Ecolumn%2Ecount=1"
        ));
        let browse = children
            .split("href=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .replace("&amp;", "&");
        let (_, full_list) = request(
            state.clone(),
            "GET",
            &browse,
            &cookie,
            &HashMap::new(),
            false,
        )
        .await;
        assert!(full_list.contains("data-label=\"count\""));
        assert!(!full_list.contains("data-label=\"Identifier\""));
        assert!(full_list.contains("id=\"columns-collection\""));
    }
    // Hidden/unknown fields cannot be exposed, and clearing every checkbox
    // keeps a usable Record column and the row's action buttons.
    let (_, fallback) = request(state.clone(), "GET", &format!("{base}?child.children.columns=custom&child.children.column.secret=1&child.children.column.unknown=1"), &cookie, &HashMap::new(), false).await;
    let children = section(&fallback, "children");
    assert!(children.contains("data-label=\"Record\""));
    assert!(!children.contains("data-label=\"secret\""));
    assert!(!children.contains("type=\"checkbox\" name=\"child.children.column.secret\""));
    assert!(children.contains("aria-label=\"Actions for First\""));
    assert_eq!(state.editor.get("groups", "g").unwrap().revision, revision);
    assert_eq!(std::fs::read(root.join(".editor/ui.json")).unwrap(), config);
}
#[tokio::test]
async fn project_selection_isolates_data_drafts_and_csrf() {
    use structured_editor_core::projects::Projects;
    let (temp, original, cookie, _) = fixture().await;
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for file in std::fs::read_dir(from).unwrap() {
            let file = file.unwrap();
            let target = to.join(file.file_name());
            if file.file_type().unwrap().is_dir() {
                copy(&file.path(), &target);
            } else {
                std::fs::copy(file.path(), target).unwrap();
            }
        }
    }
    copy(&temp.path().join("project"), &temp.path().join("second"));
    std::fs::write(
        temp.path().join("second/data/notes/one.json"),
        r#"{"name":"Second project record","count":2}"#,
    )
    .unwrap();
    let path = temp.path().join("projects.json");
    std::fs::write(
        &path,
        json!({"default":"first","projects":[
            {"id":"first","name":"First project","path":"project"},
            {"id":"second","name":"Second project","path":"second"}
        ]})
        .to_string(),
    )
    .unwrap();
    let assets = original.assets.clone();
    drop(original);
    let server = crate::projects::ProjectServer::open(
        Projects::load(&path).unwrap(),
        &temp.path().join("state"),
        assets,
        true,
    )
    .await
    .unwrap();
    let first = server.default_state();
    let mut headers = HeaderMap::new();
    headers.insert("cookie", cookie.parse().unwrap());
    let actor = first.auth.actor(&headers).await.unwrap();
    let second_actor = first.auth.scoped("second").actor(&headers).await.unwrap();
    assert_ne!(actor.csrf, second_actor.csrf);
    let draft = first
        .sessions
        .lock()
        .unwrap()
        .start(&first.editor, &actor, "notes", Some("one"))
        .unwrap();
    let app = server.router();
    async fn call(
        app: &axum::Router,
        method: &str,
        url: &str,
        cookie: &str,
        body: String,
        project: Option<&str>,
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .method(method)
            .uri(url)
            .header("cookie", cookie)
            .header("content-type", "application/x-www-form-urlencoded");
        if let Some(project) = project {
            request = request.header("x-objekte-project", project);
        }
        app.clone()
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }
    async fn body(response: axum::response::Response) -> String {
        String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }
    let html = body(call(&app, "GET", "/", &cookie, String::new(), None).await).await;
    assert!(
        html.contains("id=\"project-select\"")
            && html.contains("First project")
            && html.contains("Second project")
    );
    assert!(!html.contains("Second project record"));
    let response = call(
        &app,
        "POST",
        "/projects/select",
        &cookie,
        format!("project=second&csrf={}", actor.csrf),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("HttpOnly; SameSite=Strict")
    );
    let switched = format!("{cookie}; objekte_project=second");
    let html = body(call(&app, "GET", "/", &switched, String::new(), None).await).await;
    assert!(html.contains("Second project record"));
    assert!(html.contains("value=\"second\" selected"));
    // Even without JavaScript, a stale form cannot act in the newly selected project.
    let response = call(
        &app,
        "POST",
        "/collections/notes/records/one/edit-sessions",
        &switched,
        format!("csrf={}", actor.csrf),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    // Draft IDs never resolve in the other project's registry.
    let response = call(
        &app,
        "GET",
        &format!("/edit-sessions/{}", draft.id),
        &switched,
        String::new(),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = call(&app, "GET", "/", &switched, String::new(), Some("first")).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let response = call(
        &app,
        "POST",
        "/projects/select",
        &switched,
        format!("project=../../etc&csrf={}", second_actor.csrf),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = call(
        &app,
        "POST",
        "/projects/select",
        &switched,
        format!("project=first&csrf={}", second_actor.csrf),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let response = call(
        &app,
        "GET",
        &format!("/edit-sessions/{}", draft.id),
        &cookie,
        String::new(),
        Some("first"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        first.editor.get("notes", "one").unwrap().value["name"],
        "First"
    );
}

#[tokio::test]
async fn records_render_without_backlink_scans_and_reference_fragments_are_bounded() {
    let (temp, state, cookie, _) = fixture().await;
    let data = temp.path().join("project/data/notes");
    // A broken unrelated record proves the initial view does not index backlinks.
    std::fs::write(data.join("broken.json"), "not json").unwrap();
    let (status, html) = request(
        state.clone(),
        "GET",
        "/collections/notes/records/one",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("First") && html.contains("<record-references"));
    let section = html.split("<record-references").nth(1).unwrap();
    let tag = section.split('>').next().unwrap();
    assert!(!tag.contains(" open"));
    assert!(tag.contains(" hidden"));
    assert!(tag.contains("data-ignore-morph"));
    assert!(!section.contains("Links to this record"));
    assert!(!html.contains("Loading incoming references"));
    let (_, nested) = request(
        state.clone(),
        "GET",
        "/collections/notes/records/one?node=%2Fbody",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(!nested.contains("<record-references"));
    let (status, _) = request(
        state.clone(),
        "GET",
        "/collections/notes/records/one/references",
        "",
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = request(
        state.clone(),
        "GET",
        "/collections/notes/records/one/references",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    std::fs::remove_file(data.join("broken.json")).unwrap();
    state.editor.refresh_read_cache();
    let (status, empty) = request(
        state.clone(),
        "GET",
        "/collections/notes/records/one/references",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(empty.is_empty());
    for n in 0..105 {
        std::fs::write(data.join(format!("ref-{n:03}.json")), json!({"name":format!("Ref {n}"),"count":0,"body":"[First](/collections/notes/records/one)"}).to_string()).unwrap();
    }
    state.editor.refresh_read_cache();
    let (status, html) = request(
        state.clone(),
        "GET",
        "/collections/notes/records/one/references",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(html.matches("<tr>").count(), 26);
    assert!(
        html.contains("1–25")
            && html.contains("<strong>105</strong>")
            && html.contains("data-reference-page")
    );
    assert!(!html.contains("<main") && !html.contains("<html"));
    let (_, html) = request(
        state.clone(),
        "GET",
        "/collections/notes/records/one/references?source=notes&offset=100",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(html.matches("<tr>").count(), 6);
    assert!(html.contains("101–105") && html.contains("Previous"));
    let (status, _) = request(
        state,
        "GET",
        "/collections/notes/records/missing/references",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn incoming_tables_group_unique_records_and_share_collection_columns_and_search() {
    let (temp, _state, _, actor) = fixture().await;
    let root = temp.path().join("project");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".editor/repository.json")).unwrap())
            .unwrap();
    config["collections"]["books"] = json!({"path":"data/books","format":"json","id":"filename"});
    std::fs::write(root.join(".editor/repository.json"), config.to_string()).unwrap();
    let mut schema: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".editor/objects.schema.json")).unwrap())
            .unwrap();
    let mut book = schema["anyOf"][0].clone();
    book["x-collection"] = json!("books");
    schema["anyOf"].as_array_mut().unwrap().push(book);
    std::fs::write(root.join(".editor/objects.schema.json"), schema.to_string()).unwrap();
    std::fs::write(
        root.join(".editor/ui.json"),
        json!({"collections":{
            "notes":{"label":"Notes","display":"name","list":{"columns":["name","count"]}},
            "books":{"label":"Books","display":"name","list":{"columns":["name","body"]}}
        }})
        .to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(root.join("data/books")).unwrap();
    let link = "[Target](/collections/notes/records/one)";
    for (path, value) in [
        (
            "data/notes/a.json",
            json!({"name":"Alpha","count":1,"body":link,"items":[link,link]}),
        ),
        (
            "data/notes/b.json",
            json!({"name":"Beta","count":2,"body":link}),
        ),
        (
            "data/books/book.json",
            json!({"name":"Book title","count":3,"body":link}),
        ),
        ("settings.json", json!({"notes":link})),
    ] {
        std::fs::write(root.join(path), value.to_string()).unwrap();
    }
    let editor = structured_editor_core::Editor::open(&root).unwrap();
    let html = views::references::render(&editor, "notes", "one", &HashMap::new(), &actor).unwrap();
    assert_eq!(html.matches("<record-reference-table ").count(), 3);
    assert_eq!(html.matches("<tr>").count(), 7); // 2 notes, 1 book, 1 document + headers.
    assert!(html.contains("reference-notes-title") && html.contains("reference-books-title"));
    assert!(html.contains("data-label=\"count\"") && html.contains("data-label=\"body\""));
    assert!(html.contains("data-regex-toggle") && html.contains("data-case-toggle"));
    assert!(!html.contains("reference-list") && !html.contains("links from other records"));
    let params = HashMap::from([
        ("source".into(), "notes".into()),
        ("q".into(), "^beta$".into()),
        ("regex".into(), "1".into()),
    ]);
    let filtered = views::references::render(&editor, "notes", "one", &params, &actor).unwrap();
    assert_eq!(filtered.matches("<tr>").count(), 2);
    assert!(
        filtered.contains("Beta")
            && !filtered.contains("Alpha")
            && !filtered.contains("Book title")
    );
    let mut sorted = HashMap::from([
        ("source".into(), "notes".into()),
        ("sort".into(), "count".into()),
        ("direction".into(), "desc".into()),
    ]);
    let html = views::references::render(&editor, "notes", "one", &sorted, &actor).unwrap();
    assert!(html.find(">Beta</a>").unwrap() < html.find(">Alpha</a>").unwrap());
    sorted.insert("columns".into(), "custom".into());
    sorted.insert("column.name".into(), "1".into());
    let html = views::references::render(&editor, "notes", "one", &sorted, &actor).unwrap();
    assert!(!html.contains("data-label=\"count\""));
    let empty = HashMap::from([
        ("source".into(), "notes".into()),
        ("q".into(), "no such title".into()),
    ]);
    let html = views::references::render(&editor, "notes", "one", &empty, &actor).unwrap();
    assert!(html.contains("No matching records") && html.contains("Show All"));
    let mut custom_search = sorted.clone();
    custom_search.insert("q".into(), "Alpha".into());
    let html = views::references::render(&editor, "notes", "one", &custom_search, &actor).unwrap();
    let clear = html
        .split("data-reference-query href=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert!(
        clear.contains("columns=custom")
            && clear.contains("column%2Ename=1")
            && !clear.contains("q=")
    );
}

#[tokio::test]
async fn embedded_object_tables_preserve_positions_and_independent_views() {
    let (_temp, state, _, _) = fixture().await;
    let schema = json!({"type":"object","x-presentation":{"hideEmpty":true},"properties":{
        "blank":{"type":"string","title":"Unused description"},
        "contents":{"type":"array","title":"Contents","x-presentation":{"table":{"columns":["title","agent"],"pageSize":2,"previewSize":2}},"items":{"type":"object","properties":{
            "title":{"type":"string"},"agent":{"type":"string","x-relation":{"collection":"notes"}},"rank":{"type":"integer"}
        }}},
        "items":{"type":"array","title":"Items","x-presentation":{"table":{"columns":["name"]}},"items":{"type":"object","properties":{"name":{"type":"string"},"owner":{"type":"string"}}}}
    }});
    let value = json!({"blank":"", "contents":[
        {"title":"Zulu","agent":"one","rank":10},
        {"title":"Alpha","agent":"one","rank":2},
        {"title":"Beta","agent":"missing","rank":3}
    ],"items":[{"name":"Copy one","owner":"Library"}]});
    let draft = structured_editor_core::draft::Draft::new(&schema, &value);
    let base = "/collections/notes/records/one";
    let params = HashMap::from([
        ("array./contents.sort".into(), "rank".into()),
        ("array./contents.columns".into(), "custom".into()),
        ("array./contents.column.title".into(), "1".into()),
    ]);
    let html =
        crate::tree::preview_with_options(&state.editor, &draft, &value, base, "", &params, true);
    assert!(!html.contains("Unused description"));
    assert!(html.contains("<table class=\"data-table child-table embedded-table\""));
    assert!(html.contains("1–2 of 3"));
    assert!(html.contains("name=\"array./contents.q\""));
    assert!(!html.contains("Browse list") && !html.contains("View all"));
    assert!(html.contains("node=%2Fcontents%2F1\">Alpha"));
    assert!(html.contains("node=%2Fcontents%2F2\">Beta"));
    assert!(!html.contains("node=%2Fcontents%2F0\">Zulu"));
    assert!(
        html.contains("data-label=\"name\""),
        "Items retain their own columns"
    );
    assert!(html.contains("/edit?node=%2Fcontents%2F1"));
    let navigation = html.split("</template>").next().unwrap();
    assert!(!navigation.contains("Alpha") && !navigation.contains("Zulu"));
    assert!(navigation.contains("#array-2f636f6e74656e7473"));
    assert!(!navigation.contains("node=%2Fcontents"));
    let mut search = params.clone();
    search.insert("array./contents.q".into(), "first".into());
    let html = crate::tree::preview_with_options(
        &state.editor,
        &draft,
        &value,
        base,
        "/contents",
        &search,
        false,
    );
    assert!(
        html.contains("2 of 3 rows"),
        "Search resolves linked record names"
    );
    assert!(!html.contains("/edit?node="));
    let mut page = params;
    page.insert("array./contents.offset".into(), "2".into());
    let html = crate::tree::preview_with_options(
        &state.editor,
        &draft,
        &value,
        base,
        "/contents",
        &page,
        true,
    );
    assert!(html.contains("node=%2Fcontents%2F0\">Zulu"));
    assert!(html.contains("3–3 of 3"));
    assert_eq!(
        draft.value().unwrap(),
        value,
        "Sorting never changes stored order"
    );
}

#[tokio::test]
async fn embedded_search_uses_shared_regex_case_highlights_and_validation() {
    let (_temp, state, _, _) = fixture().await;
    let schema = json!({"type":"object","properties":{
        "contents":{"type":"array","title":"Contents","x-presentation":{"table":{"columns":["title","agent"],"pageSize":1}},"items":{"type":"object","properties":{
            "title":{"type":"string"},"agent":{"type":"string","x-relation":{"collection":"notes"}},
            "note":{"type":"string","x-content":"markdown"},"hidden":{"type":"string","x-editor":"hidden"}
        }}}
    }});
    let value = json!({"contents":[
        {"title":"Über","agent":"one","note":""},
        {"title":"über","note":""},
        {"title":"Elsewhere","agent":"one","note":"**Secret** <img src=x onerror=alert(1)>","hidden":"Invisible"}
    ]});
    let draft = structured_editor_core::draft::Draft::new(&schema, &value);
    let base = "/collections/notes/records/one";
    let mut params = HashMap::from([
        ("node".into(), "/contents".into()),
        ("array./contents.q".into(), "^Ü.er$".into()),
        ("array./contents.regex".into(), "1".into()),
        ("array./contents.case_sensitive".into(), "0".into()),
        ("array./items.q".into(), "separate".into()),
        ("array./items.regex".into(), "1".into()),
    ]);
    let render = |params: &HashMap<String, String>| {
        crate::tree::preview_with_options(
            &state.editor,
            &draft,
            &value,
            base,
            "/contents",
            params,
            true,
        )
    };
    let html = render(&params);
    assert!(html.contains("data-regex-search"));
    assert!(html.contains("data-case-toggle aria-label=\"Match case\" aria-pressed=\"false\""));
    assert!(html.contains("<mark>Über</mark>"));
    assert!(html.contains("2 of 3 rows") && html.contains("1–1 of 2"));
    let links: Vec<_> = html
        .split("href=\"")
        .skip(1)
        .map(|part| {
            percent_encoding::percent_decode_str(part.split('"').next().unwrap())
                .decode_utf8_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        links
            .iter()
            .any(|link| link.contains("array./contents.offset=1")
                && link.contains("array./contents.regex=1")
                && link.contains("array./contents.case_sensitive=0"))
    );
    assert!(
        links
            .iter()
            .any(|link| link.contains("array./contents.sort=title")
                && link.contains("array./contents.regex=1"))
    );
    for form in html
        .split("<form")
        .skip(1)
        .map(|part| part.split("</form>").next().unwrap())
    {
        assert_eq!(form.matches("name=\"array./contents.regex\"").count(), 1);
        assert_eq!(
            form.matches("name=\"array./contents.case_sensitive\"")
                .count(),
            1
        );
        assert!(form.contains("name=\"array./items.regex\" value=\"1\""));
    }
    let show_all = html
        .split(">Show All</a>")
        .next()
        .unwrap()
        .rsplit("href=\"")
        .next()
        .unwrap();
    let show_all = percent_encoding::percent_decode_str(show_all).decode_utf8_lossy();
    assert!(
        !show_all.contains("array./contents.q")
            && !show_all.contains("array./contents.regex")
            && !show_all.contains("array./contents.case_sensitive")
    );
    assert!(
        show_all.contains("array./items.q=separate") && show_all.contains("array./items.regex=1")
    );
    params.insert("array./contents.offset".into(), "1".into());
    let html = render(&params);
    assert!(html.contains("<mark>über</mark>") && html.contains("/edit?node=%2Fcontents%2F1"));
    params.remove("array./contents.offset");
    params.insert("array./contents.case_sensitive".into(), "1".into());
    assert!(render(&params).contains("1 of 3 rows"));
    params.insert("array./contents.regex".into(), "0".into());
    assert!(render(&params).contains("No matching rows"));
    params.insert("array./contents.q".into(), "über".into());
    assert!(render(&params).contains("<mark>über</mark>"));
    params.insert("array./contents.q".into(), "^First$".into());
    params.insert("array./contents.regex".into(), "1".into());
    let html = render(&params);
    assert!(html.contains("2 of 3 rows"));
    assert!(html.contains("href=\"/collections/notes/records/one\"><mark>First</mark></a>"));
    params.insert("array./contents.q".into(), "^Secret".into());
    let html = render(&params);
    assert!(html.contains("1 of 3 rows") && html.contains("class=\"match-row\""));
    assert!(html.contains("<mark>Secret</mark> &lt;img"));
    assert!(!html.contains("<img"));
    params.insert("array./contents.q".into(), "Invisible".into());
    assert!(render(&params).contains("No matching rows"));
    for pattern in ["[", "(?=a)", r"(a)\1"] {
        params.insert("array./contents.q".into(), pattern.into());
        let html = render(&params);
        assert!(html.contains("aria-invalid=\"true\"") && html.contains("regex parse error"));
        assert!(!html.contains("No matching rows"));
    }
    assert_eq!(draft.value().unwrap(), value);
}

#[tokio::test]
async fn record_tables_follow_all_details_in_preview_and_edit() {
    let schema = json!({"type":"object","properties":{
        "a_table":{"type":"array","title":"Table rows","items":{"type":"object","default":{"name":""},"properties":{"name":{"type":"string"}}}},
        "z_description":{"type":"string","title":"Description"},
        "metadata":{"type":"string","title":"Import notes","x-presentation":{"secondary":true}}
    }});
    let (temp, state, cookie, actor) = fixture_with_settings_schema(schema).await;
    let value = json!({"a_table":[{"name":"A row"}],"z_description":"Main description","metadata":"Source notes"});
    std::fs::write(temp.path().join("project/settings.json"), value.to_string()).unwrap();
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    let preview = crate::tree::preview(
        &state.editor,
        &session.draft,
        &value,
        "/documents/editor-settings?file=settings.json",
        "",
    );
    let table = preview
        .find("<section class=\"surface child-list embedded-panel\"")
        .unwrap();
    assert!(preview.find("Main description").unwrap() < table);
    assert!(preview.find("Source notes").unwrap() < table);
    assert!(preview.contains("name=\"array./a_table.q\""));
    assert!(!preview.contains("Browse list"));
    let edit = views::edit(&state.editor, &session, &actor, &state.fields, &[], "").unwrap();
    let table = edit.find("<div class=\"embedded-editor surface\"").unwrap();
    assert!(edit.find("Main description").unwrap() < table);
    assert!(edit.find("Source notes").unwrap() < table);
    let array = session.draft.at_path("/a_table").unwrap();
    let form = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), session.version.to_string()),
    ]);
    let (status, html) = request(
        state.clone(),
        "POST",
        &format!(
            "/edit-sessions/{}/fields/{}/actions/add",
            session.id, array.id
        ),
        &cookie,
        &form,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{html}");
    let mut sessions = state.sessions.lock().unwrap();
    let updated = sessions.get(&session.id, &actor).unwrap();
    assert_eq!(updated.view_path, "/a_table/1");
    let name = &updated.draft.at_path("/a_table/1/name").unwrap().id;
    assert!(html.contains(&format!("name=\"value.{name}\"")));
    assert_eq!(
        updated.value().unwrap()["z_description"],
        value["z_description"]
    );
    assert_eq!(
        state
            .editor
            .json_document("editor-settings", "settings.json")
            .unwrap()
            .value,
        value,
        "Adding a row only changes the draft"
    );
}

#[tokio::test]
async fn edit_tables_page_without_losing_draft_values_and_follow_moved_rows() {
    let schema = json!({"type":"object","properties":{
        "title":{"type":"string"},
        "contents":{"type":"array","x-presentation":{"table":{"pageSize":2}},"items":{"type":"object","default":{"name":""},"properties":{"name":{"type":"string"}}}}
    }});
    let (temp, state, cookie, actor) = fixture_with_settings_schema(schema).await;
    let value = json!({"title":"Original", "contents":[{"name":"First"},{"name":"Second"},{"name":"Third"},{"name":"Fourth"},{"name":"Fifth"}]});
    std::fs::write(temp.path().join("project/settings.json"), value.to_string()).unwrap();
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    state
        .sessions
        .lock()
        .unwrap()
        .update(&actor, session.clone())
        .unwrap();
    let array = session.draft.at_path("/contents").unwrap().id.clone();
    let title = session.draft.at_path("/title").unwrap().id.clone();
    let html = views::edit(&state.editor, &session, &actor, &state.fields, &[], "").unwrap();
    assert!(html.contains(&format!("href=\"#field-{array}\"")));
    assert!(!html.contains("Edit all 5 rows"));
    assert!(
        html.contains("</i>contents <span class=\"badge\">5</span></h2>") && html.contains("1–2 of 5")
            && html.contains("Edit row 2")
            && !html.contains("Edit row 3")
    );
    let form = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), session.version.to_string()),
        (format!("present.{title}"), "1".into()),
        (format!("value.{title}"), "Unsaved title".into()),
    ]);
    let (status, html) = request(
        state.clone(),
        "POST",
        &format!(
            "/edit-sessions/{}/fields/{array}/actions/table-page?item=2",
            session.id
        ),
        &cookie,
        &form,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        html.contains("3–4 of 5")
            && html.contains("Edit row 3")
            && !html.contains("Edit row 2")
    );
    session = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert_eq!(session.value().unwrap()["title"], "Unsaved title");
    assert_eq!(session.value().unwrap()["contents"], value["contents"]);
    let item = session.draft.at_path("/contents/2").unwrap().id.clone();
    let form = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), session.version.to_string()),
    ]);
    let (_, html) = request(
        state.clone(),
        "POST",
        &format!(
            "/edit-sessions/{}/fields/{array}/actions/up?item={item}",
            session.id
        ),
        &cookie,
        &form,
        false,
    )
    .await;
    assert!(html.contains("1–2 of 5") && html.contains("Third"));
    session = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert_eq!(session.value().unwrap()["contents"][1]["name"], "Third");
    let form = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), session.version.to_string()),
    ]);
    request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/undo", session.id),
        &cookie,
        &form,
        false,
    )
    .await;
    session = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert_eq!(session.value().unwrap()["contents"], value["contents"]);
    assert_eq!(session.value().unwrap()["title"], "Unsaved title");
    assert_eq!(
        state
            .editor
            .json_document("editor-settings", "settings.json")
            .unwrap()
            .value,
        value
    );
}

#[tokio::test]
async fn edit_tables_search_in_parent_and_keep_unsaved_fields() {
    let schema = json!({"type":"object","properties":{
        "title":{"type":"string"},
        "contents":{"type":"array","x-presentation":{"table":{"pageSize":1}},"items":{"type":"object","properties":{"name":{"type":"string"}}}}
    }});
    let (temp, state, cookie, actor) = fixture_with_settings_schema(schema).await;
    let value = json!({"title":"Original","contents":[{"name":"Alpha"},{"name":"Beta"},{"name":"Bravo"}]});
    std::fs::write(temp.path().join("project/settings.json"), value.to_string()).unwrap();
    let mut session = state.sessions.lock().unwrap().start_document(&state.editor, &actor, "editor-settings", "settings.json").unwrap();
    let array = session.draft.at_path("/contents").unwrap().id.clone();
    let title = session.draft.at_path("/title").unwrap().id.clone();
    let mut form = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), session.version.to_string()),
        (format!("present.{title}"), "1".into()),
        (format!("value.{title}"), "Draft title".into()),
        (format!("table-query.{array}"), "^B".into()),
        (format!("table-regex.{array}"), "1".into()),
    ]);
    let (status, html) = request(state.clone(), "POST", &format!("/edit-sessions/{}/fields/{array}/actions/table-search", session.id), &cookie, &form, false).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("1–1 of 2") && html.contains("Beta") && !html.contains("Edit row 1"));
    session = state.sessions.lock().unwrap().get(&session.id, &actor).unwrap().clone();
    assert!(session.view_path.is_empty());
    assert_eq!(session.value().unwrap()["title"], "Draft title");
    assert_eq!(state.editor.json_document("editor-settings", "settings.json").unwrap().value, value);
    form.insert("version".into(), session.version.to_string());
    let (status, html) = request(state.clone(), "POST", &format!("/edit-sessions/{}/fields/{array}/actions/table-page?item=1", session.id), &cookie, &form, false).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("2–2 of 2") && html.contains("Bravo"));
    session = state.sessions.lock().unwrap().get(&session.id, &actor).unwrap().clone();
    assert!(session.view_path.is_empty());
}

#[tokio::test]
async fn edit_forms_keep_required_fields_visible_and_resolve_nested_reference_names() {
    let schema = json!({"type":"object","required":["required","catalogue_id"],"properties":{
        "required":{"type":"string","title":"Required name","x-presentation":{"secondary":true,"hideEmpty":true}},
        "catalogue_id":{"type":"integer","title":"Catalogue ID","x-presentation":{"secondary":true}},
        "optional":{"type":"string","x-presentation":{"hideEmpty":true}},
        "contents":{"type":"array","title":"Contents","items":{"type":"object","properties":{
            "title":{"type":"string"},"agents":{"type":"array","title":"Agents","items":{"type":"object","properties":{"agent":{"type":"string","title":"Name","x-relation":{"collection":"notes"}}}}}
        }}}
    }});
    let (temp, state, _, actor) = fixture_with_settings_schema(schema).await;
    let value = json!({"required":"", "optional":"", "contents":[{"title":"A poem", "agents":[{"agent":"one"}]}]});
    std::fs::write(temp.path().join("project/settings.json"), value.to_string()).unwrap();
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    let html = views::edit(&state.editor, &session, &actor, &state.fields, &[], "").unwrap();
    assert!(html.find("Required name").unwrap() < html.find("More fields").unwrap());
    let required_id = session.draft.at_path("/catalogue_id").unwrap();
    assert!(html.contains(&format!("name=\"value.{}\"", required_id.id)));
    assert!(html.find("Catalogue ID").unwrap() < html.find("More fields").unwrap());
    assert!(
        !html.contains("Remove Required name field") && !html.contains("aria-label=\"required\"")
    );
    session.view_path = "/contents/0/agents/0".into();
    let html = views::edit(&state.editor, &session, &actor, &state.fields, &[], "").unwrap();
    let crumbs = html
        .split("aria-label=\"Editing location\"")
        .nth(1)
        .unwrap()
        .split("</nav>")
        .next()
        .unwrap();
    assert!(
        crumbs.contains("Overview")
            && crumbs.contains("Contents")
            && crumbs.contains("A poem")
            && crumbs.contains("Agents")
            && crumbs.contains("First"),
        "{crumbs}"
    );
    assert!(html.contains("class=\"reference-name\"") && html.contains(">First <i"));
    let field = session.draft.at_path("/contents/0/agents/0/agent").unwrap();
    assert!(html.contains(&format!("name=\"value.{}\" value=\"one\"", field.id)));
    assert!(html.contains("Enter ID manually") && html.contains("data-reference-picker"));
    assert_eq!(session.value().unwrap(), value);
}

#[tokio::test]
async fn project_table_page_size_applies_to_read_and_edit_and_field_overrides_win() {
    let schema = json!({"type":"object","properties":{
        "tablePageSize":{"type":"integer","minimum":1,"maximum":100},
        "contents":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"}}}}
    }});
    let (temp, state, _, actor) = fixture_with_settings_schema(schema).await;
    let value = json!({"tablePageSize":2, "contents":[{"name":"First"},{"name":"Second"},{"name":"Third"}]});
    std::fs::write(temp.path().join("project/settings.json"), value.to_string()).unwrap();
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    session.view_path = "/contents".into();
    let html = views::edit(&state.editor, &session, &actor, &state.fields, &[], "").unwrap();
    assert!(
        html.contains("1–2 of 3") && !html.contains("Edit row 3"),
        "{html}"
    );
    let preview = crate::tree::preview_with_options(
        &state.editor,
        &session.draft,
        &value,
        "/documents/editor-settings?file=settings.json",
        "/contents",
        &HashMap::from([("node".into(), "/contents".into())]),
        true,
    );
    assert!(
        preview.contains("First") && preview.contains("Second") && !preview.contains("Third"),
        "{preview}"
    );
    let mut field = session.draft.at_path("/contents").unwrap().clone();
    assert_eq!(views::embedded::edit_page_size(&state.editor, &field), 2);
    field.schema["x-presentation"] = json!({"table":{"pageSize":3}});
    assert_eq!(views::embedded::edit_page_size(&state.editor, &field), 3);
}

#[tokio::test]
async fn musenalm_settings_offer_named_controls_and_keep_personal_choices_separate() {
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/musenalm/.editor/settings.schema.json"
    ))
    .unwrap();
    let (_temp, state, cookie, actor) = fixture_with_settings_schema(schema).await;
    let (_, html) = request(
        state.clone(),
        "GET",
        "/settings",
        &cookie,
        &HashMap::new(),
        false,
    )
    .await;
    assert!(html.contains("Your appearance") && html.contains("Project defaults"));
    assert!(html.contains("Rows per table page") && html.contains("Follow device"));
    assert!(html.find("data-personal-theme").unwrap() < html.find("id=\"document-form\"").unwrap());
    let mut reader = actor.clone();
    reader.role = "user".into();
    let html = crate::preferences::render(&state.editor, &reader, "{}", "revision", "", "success")
        .unwrap()
        .unwrap();
    assert!(html.contains("value=\"validate\" disabled"));
    assert!(html.contains("value=\"save\" disabled"));
    let saved = state.editor.settings().unwrap();
    let value = json!({"defaultTheme":"system"});
    state
        .editor
        .save_settings(value.clone(), &saved.revision)
        .unwrap();
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    let edit = views::edit(&state.editor, &session, &actor, &state.fields, &[], "").unwrap();
    assert!(edit.contains("Follow device") && edit.contains("&quot;system&quot;"));
    let preview = crate::tree::preview(
        &state.editor,
        &session.draft,
        &value,
        "/documents/editor-settings?file=settings.json",
        "",
    );
    assert!(preview.contains("Follow device"));
    assert_eq!(session.value().unwrap(), value);
}

#[tokio::test]
async fn schema_choice_arrays_share_multiselects_and_preserve_order_and_identity() {
    let schema = json!({"type":"object","$defs":{"choice":{"type":"string","enum":["deu","eng","fra",""],"x-options":[]}},"properties":{
        "audiences":{"type":"array","items":{"type":"string","x-relation":{"collection":"notes"}}},
        "locales":{"type":"array","items":{"$ref":"#/$defs/choice"}},
        "flags":{"type":"array","items":{"enum":[0,1,false]}},
        "locked":{"type":"array","x-ownership":"immutable","items":{"enum":["x","y"]}}
    }});
    let (temp, state, _, actor) = fixture_with_settings_schema(schema).await;
    std::fs::write(
        temp.path().join("project/settings.json"),
        json!({"audiences":["one"],"locales":[],"flags":[0],"locked":["x"]}).to_string(),
    )
    .unwrap();
    let mut session = state
        .sessions
        .lock()
        .unwrap()
        .start_document(&state.editor, &actor, "editor-settings", "settings.json")
        .unwrap();
    let html = views::edit(&state.editor, &session, &actor, &state.fields, &[], "").unwrap();
    assert!(html.contains("array-choices") && html.contains("First"));
    assert!(!html.contains("Enter ID manually"));
    let locales = session.draft.at_path("/locales").unwrap().id.clone();
    crate::pickers::action(
        &state.editor,
        &mut session,
        &locales,
        "multiple",
        &HashMap::new(),
    )
    .unwrap();
    for choice in ["\"deu\"", "\"fra\""] {
        crate::pickers::action(
            &state.editor,
            &mut session,
            &locales,
            "toggle-choice",
            &HashMap::from([("choice".into(), choice.into())]),
        )
        .unwrap();
        assert!(session.picker.is_some(), "Choose several without reopening");
    }
    let id = session.draft.at_path("/locales/0").unwrap().id.clone();
    let mut form = HashMap::from([("choice".into(), "\"fra\"".into())]);
    crate::pickers::action(
        &state.editor,
        &mut session,
        &locales,
        "toggle-choice",
        &form,
    )
    .unwrap();
    assert_eq!(session.value().unwrap()["locales"], json!(["deu"]));
    assert_eq!(session.draft.at_path("/locales/0").unwrap().id, id);
    form.insert("choice".into(), "\"not-an-option\"".into());
    assert!(
        crate::pickers::action(
            &state.editor,
            &mut session,
            &locales,
            "toggle-choice",
            &form
        )
        .is_err()
    );
    let picker = crate::pickers::render(&state.editor, &session, &actor).unwrap();
    assert!(
        picker.contains("role=\"checkbox\" aria-checked=\"true\"")
            && picker.contains(">Done</button>")
    );
    assert!(picker.contains("Empty value"));
    let audiences = session.draft.at_path("/audiences").unwrap().id.clone();
    crate::pickers::action(
        &state.editor,
        &mut session,
        &audiences,
        "multiple",
        &HashMap::new(),
    )
    .unwrap();
    assert!(
        crate::pickers::render(&state.editor, &session, &actor)
            .unwrap()
            .contains("First")
    );
    form.insert("choice".into(), "\"missing-record\"".into());
    assert!(
        crate::pickers::action(
            &state.editor,
            &mut session,
            &audiences,
            "toggle-choice",
            &form
        )
        .is_err()
    );
    let locked = session.draft.at_path("/locked").unwrap().id.clone();
    assert!(
        crate::pickers::action(&state.editor, &mut session, &locked, "multiple", &form).is_err()
    );
    let flags = session.draft.at_path("/flags").unwrap().id.clone();
    crate::pickers::action(
        &state.editor,
        &mut session,
        &flags,
        "multiple",
        &HashMap::new(),
    )
    .unwrap();
    form.insert("choice".into(), "false".into());
    crate::pickers::action(&state.editor, &mut session, &flags, "toggle-choice", &form).unwrap();
    assert_eq!(session.value().unwrap()["flags"], json!([0, false]));
}

#[tokio::test]
async fn drag_moves_use_version_checks_capture_input_and_undo() {
    let (_temp, state, cookie, actor) = fixture().await;
    let session = state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, &actor, "notes", Some("one"))
        .unwrap();
    let array = session.draft.at_path("/items").unwrap().id.clone();
    let first = session.draft.at_path("/items/0").unwrap().id.clone();
    let second = session.draft.at_path("/items/1").unwrap().id.clone();
    let form = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), session.version.to_string()),
        ("array_before".into(), first.clone()),
        (format!("present.{second}"), "1".into()),
        (format!("value.{second}"), "unsaved".into()),
    ]);
    let url = format!(
        "/edit-sessions/{}/fields/{array}/actions/move?item={second}",
        session.id
    );
    let (status, html) = request(state.clone(), "POST", &url, &cookie, &form, false).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    let updated = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert_eq!(updated.value().unwrap()["items"], json!(["unsaved", "a"]));
    assert_eq!(updated.draft.at_path("/items/0").unwrap().id, second);
    assert_eq!(
        request(state.clone(), "POST", &url, &cookie, &form, false)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let form = HashMap::from([
        ("csrf".into(), actor.csrf.clone()),
        ("version".into(), updated.version.to_string()),
    ]);
    let (status, _) = request(
        state.clone(),
        "POST",
        &format!("/edit-sessions/{}/undo", session.id),
        &cookie,
        &form,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let restored = state
        .sessions
        .lock()
        .unwrap()
        .get(&session.id, &actor)
        .unwrap()
        .clone();
    assert_eq!(restored.value().unwrap()["items"], json!(["a", "unsaved"]));
    assert_eq!(
        state.editor.get("notes", "one").unwrap().value["items"],
        json!(["a", "b"])
    );
}

#[tokio::test]
async fn object_labels_follow_presentation_in_unrelated_schemas() {
    let (_temp, state, _, _) = fixture().await;
    let schema = json!({"properties":{
        "aaa":{"type":"string"},"caption":{"type":"string","x-presentation":{"primary":true}},
        "holder":{"type":"string","x-relation":{"collection":"notes"}}
    },"x-presentation":{"fieldOrder":["holder","caption","aaa"]}});
    let mut cells = views::embedded::Cells::new(&state.editor);
    assert_eq!(
        cells.text(
            &schema,
            &json!({"aaa":"Noise","caption":"Displayed","holder":"one"})
        ),
        "Displayed"
    );
    let html = cells.html(&schema, &json!({"aaa":"Noise","caption":"","holder":"one"}));
    assert!(html.contains(">First</a>"), "{html}");
}

#[tokio::test]
async fn interface_language_preserves_unsaved_values_and_uses_configured_labels() {
    let (temp, original, cookie, actor) = fixture().await;
    assert!(!views::shell(&original, &actor, "").unwrap().contains("data-language-picker"));
    let root = temp.path().join("project");
    std::fs::write(root.join(".editor/ui.json"), json!({
        "i18n":{"defaultLanguage":"en","languages":{"en":"English","de":"Deutsch"}},
        "collections":{"notes":{"label":"Notes","display":"name","translations":{"de":{"label":"Notizen","singularLabel":"Notiz"}},
          "fields":{"/properties/name":{"title":"Title","translations":{"de":{"title":"Titel","description":"Eine Beschreibung"}}}}
        }}
    }).to_string()).unwrap();
    let state = AppState::for_project(Editor::open(&root).unwrap(), original.auth.clone(), original.assets.clone(), "default".into(), Arc::new(vec![]));
    let session=state.sessions.lock().unwrap().start(&state.editor,&actor,"notes",Some("one")).unwrap();
    let mut input=form(&session,&actor);
    input.insert("ui_language".into(),"de".into());
    input.insert(format!("value.{}", session.draft.at_path("/name").unwrap().id),"Save changes".into());
    input.insert(format!("value.{}", session.draft.at_path("/count").unwrap().id),"unfinished".into());
    let (status,html)=request(state.clone(),"POST",&format!("/edit-sessions/{}/language",session.id),&cookie,&input,true).await;
    assert_eq!(status,StatusCode::OK,"{html}");
    assert!(html.contains("Änderungen speichern") && html.contains("Notizen") && html.contains("Titel") && html.contains("Eine Beschreibung"));
    assert!(html.contains("value=\"Save changes\""), "User data must not be translated");
    let draft=state.sessions.lock().unwrap().get(&session.id,&actor).unwrap().clone();
    assert_eq!(draft.draft.at_path("/count").unwrap().raw,"unfinished");
    assert_eq!(draft.draft.at_path("/items/0").unwrap().id,session.draft.at_path("/items/0").unwrap().id);
    assert_eq!(draft.version,session.version+1);
    assert_eq!(state.editor.get("notes","one").unwrap().value["name"],"First");
    let (status,_)=request(state.clone(),"POST",&format!("/edit-sessions/{}/language",session.id),&cookie,&input,true).await;
    assert_eq!(status,StatusCode::CONFLICT);
    let (_,german)=request(state.clone(),"GET","/collections/notes",&format!("{cookie}; objekte_language=de"),&HashMap::new(),false).await;
    assert!(german.contains("<html lang=\"de\">") && german.contains("Notizen") && german.contains("Titel"));
    let (_,english)=request(state,"GET","/collections/notes",&cookie,&HashMap::new(),false).await;
    assert!(english.contains("<html lang=\"en\">") && !english.contains("Notizen"),"Locale must not leak across requests");
}

#[tokio::test]
async fn dropdown_search_is_read_only_and_selection_keeps_its_query() {
    let schema=json!({"type":"object","properties":{"choices":{"type":"array","items":{"enum":["red","blue","green"],"x-ui-translations":{"de":{"x-optionLabels":{"red":"Rot","blue":"Blau","green":"Grün"}}}}}}});
    let (_temp,state,cookie,actor)=fixture_with_settings_schema(schema).await;
    let mut session=state.sessions.lock().unwrap().start_document(&state.editor,&actor,"editor-settings","settings.json").unwrap();
    let field=session.draft.at_path("/choices").unwrap().id.clone();
    crate::pickers::action(&state.editor,&mut session,&field,"multiple",&HashMap::new()).unwrap();
    state.sessions.lock().unwrap().update(&actor,session.clone()).unwrap();
    let url=format!("/edit-sessions/{}/fields/{field}/choices?q=blu",session.id);
    let (status,html)=request(state.clone(),"GET",&url,&cookie,&HashMap::new(),false).await;
    assert_eq!(status,StatusCode::OK);assert!(html.contains("data-popover") && html.contains(">blue</span>") && !html.contains(">red</span>"));
    let stored=state.sessions.lock().unwrap().get(&session.id,&actor).unwrap().clone();
    assert_eq!(stored.version,session.version);assert_eq!(stored.picker.unwrap().query,"");
    let mut input=form(&session,&actor);input.insert("picker_query".into(),"blu".into());input.insert("choice".into(),"\"blue\"".into());
    let (status,html)=request(state.clone(),"POST",&format!("/edit-sessions/{}/fields/{field}/actions/toggle-choice",session.id),&cookie,&input,false).await;
    assert_eq!(status,StatusCode::OK,"{html}");
    let stored=state.sessions.lock().unwrap().get(&session.id,&actor).unwrap().clone();
    assert_eq!(stored.value().unwrap()["choices"],json!(["blue"]));assert_eq!(stored.picker.as_ref().unwrap().query,"blu");
    let mut german=stored;german.picker.as_mut().unwrap().query="grü".into();
    let translated=crate::i18n::with_language("de",||crate::pickers::render(&state.editor,&german,&actor).unwrap());
    assert!(translated.contains(">Grün</span>") && !translated.contains(">Blau</span>"));
}

#[tokio::test]
async fn adding_an_empty_optional_field_reveals_it_in_the_form() {
    let schema=json!({"type":"object","properties":{"places":{"type":"array","x-presentation":{"hideEmpty":true},"items":{"enum":["Berlin","Paris"]}}}});
    let (_temp,state,cookie,actor)=fixture_with_settings_schema(schema).await;
    let session=state.sessions.lock().unwrap().start_document(&state.editor,&actor,"editor-settings","settings.json").unwrap();
    let field=session.draft.at_path("/places").unwrap().id.clone();
    let (status,html)=request(state.clone(),"POST",&format!("/edit-sessions/{}/fields/{field}/actions/include",session.id),&cookie,&form(&session,&actor),false).await;
    assert_eq!(status,StatusCode::OK,"{html}");
    assert!(html.contains("data-select-anchor") && !html.contains("<summary>More fields</summary>"));
    assert!(!html.contains("<small>array</small>"));
    let current=state.sessions.lock().unwrap().get(&session.id,&actor).unwrap().clone();
    assert!(current.revealed_fields.contains(&field));
    let (_,html)=request(state,"POST",&format!("/edit-sessions/{}/fields/{field}/actions/multiple",session.id),&cookie,&form(&current,&actor),false).await;
    assert!(html.contains("data-popover") && !html.contains("<summary>More fields</summary>"));
}

#[tokio::test]
async fn autocomplete_reads_without_opening_and_only_adds_existing_choices() {
    let schema = json!({"type":"object","required":["languages","places"],"properties":{
        "languages":{"type":"array","items":{"enum":["deu","eng"]}},
        "places":{"type":"array","items":{"type":"string","x-relation":{"collection":"notes"}}},
        "locked":{"type":"array","x-ownership":"immutable","items":{"enum":["x"]}}
    }});
    let (temp,state,cookie,actor)=fixture_with_settings_schema(schema).await;
    std::fs::write(temp.path().join("project/settings.json"), json!({"languages":[],"places":[]}).to_string()).unwrap();
    let mut session=state.sessions.lock().unwrap().start_document(&state.editor,&actor,"editor-settings","settings.json").unwrap();
    let field=session.draft.at_path("/languages").unwrap().id.clone();
    let (status,html)=request(state.clone(),"GET",&format!("/edit-sessions/{}/fields/{field}/choices?q=de",session.id),&cookie,&HashMap::new(),false).await;
    assert_eq!(status,StatusCode::OK,"{html}");assert!(html.contains(">deu</span>") && !html.contains(">eng</span>"));
    let stored=state.sessions.lock().unwrap().get(&session.id,&actor).unwrap().clone();
    assert_eq!(stored.version,session.version);assert!(stored.picker.is_none());
    let html=views::edit(&state.editor,&session,&actor,&state.fields,&[],"").unwrap();
    assert!(html.contains("role=\"combobox\"") && html.contains(">Browse</span>"));
    for (path,value,valid) in [("/languages","deu",true),("/languages","invented",false),("/places","one",true),("/places","missing",false),("/locked","x",false)] {
        let id=session.draft.at_path(path).unwrap().id.clone();
        let choice=HashMap::from([("choice".into(),json!(value).to_string())]);
        assert_eq!(crate::pickers::action(&state.editor,&mut session,&id,"select-choice",&choice).is_ok(),valid);
        assert!(session.picker.is_none());
    }
    let id=session.draft.at_path("/languages/0").unwrap().id.clone();
    crate::pickers::action(&state.editor,&mut session,&field,"select-choice",&HashMap::from([("choice".into(),"\"deu\"".into())])).unwrap();
    assert_eq!(session.value().unwrap()["languages"],json!(["deu"]));
    assert_eq!(session.draft.at_path("/languages/0").unwrap().id,id);
    assert_eq!(session.value().unwrap()["places"],json!(["one"]));
    // Exercise the version-checked route too, with no open picker.
    let mut input=form(&stored,&actor);input.insert("choice".into(),"\"eng\"".into());
    let (status,html)=request(state.clone(),"POST",&format!("/edit-sessions/{}/fields/{field}/actions/select-choice",session.id),&cookie,&input,false).await;
    assert_eq!(status,StatusCode::OK,"{html}");
    let stored=state.sessions.lock().unwrap().get(&session.id,&actor).unwrap().clone();
    assert_eq!(stored.value().unwrap()["languages"],json!(["eng"]));assert!(stored.picker.is_none());
}

#[tokio::test]
async fn local_choice_input_is_validated_preserves_identity_and_only_save_writes() {
    let schema=json!({"type":"object","properties":{
        "languages":{"type":"array","maxItems":3,"items":{"enum":["deu","eng","fra"]}},
        "places":{"type":"array","items":{"type":"string","x-relation":{"collection":"notes"}}},
        "locked":{"type":"array","x-ownership":"immutable","items":{"enum":["x"]}}
    }});
    let (temp,state,cookie,actor)=fixture_with_settings_schema(schema).await;
    let path=temp.path().join("project/settings.json");
    let original=json!({"languages":["deu","eng"],"places":[],"locked":["x"]});
    std::fs::write(&path, original.to_string()).unwrap();
    let session=state.sessions.lock().unwrap().start_document(&state.editor,&actor,"editor-settings","settings.json").unwrap();
    let field=session.draft.at_path("/languages").unwrap().id.clone();
    let places=session.draft.at_path("/places").unwrap().id.clone();
    let locked=session.draft.at_path("/locked").unwrap().id.clone();
    let first=session.draft.at_path("/languages/0").unwrap().id.clone();
    let second=session.draft.at_path("/languages/1").unwrap().id.clone();
    let mut input=form(&session,&actor);
    input.insert(format!("choices.{field}"),json!(["eng","fra","deu"]).to_string());
    input.insert(format!("choices.{places}"),json!(["one"]).to_string());
    let mut clone=session.clone();clone.apply(&state.editor,&input).unwrap();
    assert_eq!(clone.draft.at_path("/languages/0").unwrap().id,second);
    assert_eq!(clone.draft.at_path("/languages/2").unwrap().id,first);
    assert_eq!(clone.value().unwrap()["places"],json!(["one"]));
    for (id, values) in [(&field,json!(["bogus"])),(&field,json!(["eng","eng"])),(&field,json!(["deu","eng","fra","fra"])),(&places,json!(["missing"])),(&locked,json!([]))] {
        let mut bad=form(&session,&actor);bad.insert(format!("choices.{id}"),values.to_string());
        let mut clone=session.clone();assert!(clone.apply(&state.editor,&bad).is_err());
        assert_eq!(clone.value().unwrap(),original);assert_eq!(clone.version,session.version);
    }
    let (status,html)=request(state.clone(),"POST",&format!("/edit-sessions/{}/live-validation",session.id),&cookie,&input,true).await;
    assert_eq!(status,StatusCode::OK,"{html}");
    let stored=state.sessions.lock().unwrap().get(&session.id,&actor).unwrap().clone();
    assert_eq!(stored.version,session.version);assert_eq!(stored.value().unwrap(),original);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&path).unwrap()).unwrap(),original);
    // Opening Browse captures local input into the unsaved draft, without writing the record.
    let (status,html)=request(state.clone(),"POST",&format!("/edit-sessions/{}/fields/{field}/actions/multiple",session.id),&cookie,&input,false).await;
    assert_eq!(status,StatusCode::OK,"{html}");
    let stored=state.sessions.lock().unwrap().get(&session.id,&actor).unwrap().clone();
    assert_eq!(stored.value().unwrap()["languages"],json!(["eng","fra","deu"]));
    assert_eq!(serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&path).unwrap()).unwrap(),original);
    let (status,html)=request(state.clone(),"POST",&format!("/edit-sessions/{}/undo",session.id),&cookie,&form(&stored,&actor),false).await;
    assert_eq!(status,StatusCode::OK,"{html}");
    let stored=state.sessions.lock().unwrap().get(&session.id,&actor).unwrap().clone();assert_eq!(stored.value().unwrap(),original);
    let mut input=form(&stored,&actor);input.insert(format!("choices.{field}"),json!(["fra"]).to_string());
    let (status,html)=request(state.clone(),"POST",&format!("/edit-sessions/{}/save",session.id),&cookie,&input,false).await;
    assert_eq!(status,StatusCode::OK,"{html}");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&path).unwrap()).unwrap()["languages"],json!(["fra"]));
}

#[tokio::test]
async fn local_array_structure_keeps_nested_input_and_only_save_writes() {
    let (_temp,state,cookie,actor)=fixture().await;
    let session=state.sessions.lock().unwrap().start(&state.editor,&actor,"notes",Some("one")).unwrap();
    let array=session.draft.at_path("/items").unwrap().id.clone();
    let first=session.draft.at_path("/items/0").unwrap().id.clone();
    let second=session.draft.at_path("/items/1").unwrap().id.clone();
    let mut input=form(&session,&actor);
    input.insert(format!("structure.{array}"),json!([second,first]).to_string());
    input.insert(format!("value.{first}"),"unsaved nested text".into());
    let mut changed=session.clone();changed.apply(&state.editor,&input).unwrap();
    assert_eq!(changed.draft.at_path("/items/0").unwrap().id,second);
    assert_eq!(changed.draft.at_path("/items/1").unwrap().id,first);
    assert_eq!(changed.value().unwrap()["items"],json!(["b","unsaved nested text"]));
    for invalid in [json!(["unknown"]),json!([first,first]),json!([second,first,"extra"])] {
        let mut bad=form(&session,&actor);bad.insert(format!("structure.{array}"),invalid.to_string());
        let mut clone=session.clone();assert!(clone.apply(&state.editor,&bad).is_err());
        assert_eq!(clone.value().unwrap()["items"],json!(["a","b"]));
    }
    let (status,html)=request(state.clone(),"POST",&format!("/edit-sessions/{}/live-validation",session.id),&cookie,&input,true).await;
    assert_eq!(status,StatusCode::OK,"{html}");
    assert_eq!(state.sessions.lock().unwrap().get(&session.id,&actor).unwrap().version,session.version);
    assert_eq!(state.editor.get("notes","one").unwrap().value["items"],json!(["a","b"]));
    let (status,html)=request(state.clone(),"POST",&format!("/edit-sessions/{}/fields/{array}/actions/browse",session.id),&cookie,&input,false).await;
    assert_eq!(status,StatusCode::OK,"{html}");
    let stored=state.sessions.lock().unwrap().get(&session.id,&actor).unwrap().clone();
    assert_eq!(stored.value().unwrap()["items"],json!(["b","unsaved nested text"]));
    assert_eq!(state.editor.get("notes","one").unwrap().value["items"],json!(["a","b"]));
    let mut removed=form(&stored,&actor);removed.insert(format!("structure.{array}"),json!([second]).to_string());
    let (status,html)=request(state.clone(),"POST",&format!("/edit-sessions/{}/save",session.id),&cookie,&removed,false).await;
    assert_eq!(status,StatusCode::OK,"{html}");
    assert_eq!(state.editor.get("notes","one").unwrap().value["items"],json!(["b"]));
}

#[tokio::test]
async fn embedded_tables_share_configured_and_related_collection_icons() {
    let schema=json!({"type":"object","properties":{
        "custom":{"type":"array","x-presentation":{"table":{"icon":"ri-archive-line"}},"items":{"type":"object","properties":{"name":{"type":"string"}}}},
        "linked":{"type":"array","items":{"type":"object","properties":{"note":{"type":"string","x-relation":{"collection":"notes"}}}}},
        "plain":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"}}}}
    }});
    let (temp,_state,_,_)=fixture_with_settings_schema(schema.clone()).await;
    let root=temp.path().join("project");
    std::fs::write(root.join(".editor/ui.json"),json!({"collections":{"notes":{"label":"Notes","display":"name","icon":"ri-book-line"}}}).to_string()).unwrap();
    let editor=structured_editor_core::Editor::open(&root).unwrap();
    let value=json!({"custom":[{"name":"A"}],"linked":[{"note":"one"}],"plain":[{"name":"B"}]});
    let draft=structured_editor_core::draft::Draft::new(&schema,&value);
    let custom=draft.at_path("/custom").unwrap();
    let linked=draft.at_path("/linked").unwrap();
    let plain=draft.at_path("/plain").unwrap();
    assert_eq!(views::embedded::table_icon(&editor,custom),"ri-archive-line");
    assert_eq!(views::embedded::table_icon(&editor,linked),"ri-book-line");
    assert_eq!(views::embedded::table_icon(&editor,plain),"ri-table-line");
    let preview=views::embedded::render(&editor,linked,&value["linked"],"/documents/editor-settings?file=settings.json","/linked",&HashMap::new(),true);
    assert!(preview.contains("<i class=\"ri-book-line\" aria-hidden=\"true\"></i>"));
}
