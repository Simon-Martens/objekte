use super::*;
use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::json;
use std::collections::HashMap;
use tower::ServiceExt;

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
    let status = response.status();
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
    assert!(body.contains("Saved to disk"));
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
        .split("data-url=\"/edit-sessions/")
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
    let url = format!("/edit-sessions/{}/live-validation", session.id);
    let mut values = form(&session, &actor);
    values.insert(format!("value.{count}"), "-3".into());
    let (status, body) = request(state.clone(), "POST", &url, &cookie, &values, true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(&format!("id=\"errors-{count}\"")));
    assert!(body.contains("/count"));
    assert!(body.contains("issue(s) to resolve"));
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
    let html = views::list::highlighted("<script>İstanbul & HERDER</script>", "herder", true);
    assert!(html.contains("<mark>HERDER</mark>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>"));
    assert_eq!(
        views::list::highlighted("Über", "über", false),
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
        assert_eq!(response.status(), expected);
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

#[tokio::test]
async fn hierarchy_routes_render_scope_create_attach_and_reject_stale_changes() {
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
        ui["collections"][name] =
            json!({"label":name,"display":"name","showInSidebar":name=="places"});
    }
    write(".editor/repository.json", &config);
    write(".editor/objects.schema.json", &schemas);
    write(".editor/ui.json", &ui);
    Arc::get_mut(&mut state).unwrap().editor = Editor::open(&root).unwrap();
    let g = state
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
    assert!(!html.contains("All in "));
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
        .split("data-url=\"/edit-sessions/")
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
    assert!(html.contains("data-url=\"/collections/groups/records/g\""));
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
