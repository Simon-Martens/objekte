use super::*;
use axum::{Router, response::Response};

async fn call(
    app: &Router,
    method: &str,
    url: &str,
    cookie: &str,
    form: &HashMap<String, String>,
    partial: bool,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(url)
        .header("cookie", cookie)
        .header("content-type", "application/x-www-form-urlencoded");
    if partial {
        request = request.header("datastar-request", "true");
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(serde_urlencoded::to_string(form).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn body(response: Response) -> String {
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

#[tokio::test]
async fn projects_share_login_but_isolate_urls_content_drafts_and_saves() {
    let (temp, state, cookie, actor) = fixture().await;
    let (_other_temp, other, _, _) = fixture().await;
    std::fs::write(
        other.editor.config().root.join("data/notes/one.json"),
        r#"{"name":"Second","count":2}"#,
    )
    .unwrap();
    let catalog_path = temp.path().join("projects.json");
    std::fs::write(
        &catalog_path,
        json!({"default":"first","projects":[
            {"id":"first","label":"First repository","path":state.editor.config().root},
            {"id":"second","label":"Second repository","path":other.editor.config().root}
        ]})
        .to_string(),
    )
    .unwrap();
    let catalog = crate::projects::Catalog::load(&catalog_path).unwrap();
    let app = crate::projects::router(catalog, &temp.path().join("state"), state.clone())
        .await
        .unwrap();
    let empty = HashMap::new();
    let response = call(&app, "GET", "/collections/notes?q=one", "", &empty, false).await;
    assert_eq!(
        response.headers()["location"],
        "/projects/first/collections/notes?q=one"
    );
    let response = call(
        &app,
        "GET",
        "/projects/second/collections/notes",
        "",
        &empty,
        false,
    )
    .await;
    assert_eq!(
        response.headers()["location"],
        "/projects/second/auth/login"
    );
    assert_eq!(
        call(&app, "GET", "/projects/missing/", &cookie, &empty, false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    for (project, expected, unexpected) in
        [("first", "First", "Second"), ("second", "Second", "First")]
    {
        for partial in [false, true] {
            let response = call(
                &app,
                "GET",
                &format!("/projects/{project}/collections/notes"),
                &cookie,
                &empty,
                partial,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let html = body(response).await;
            assert!(html.contains(&format!(">{expected}</a>")), "{html}");
            assert!(!html.contains(&format!(">{unexpected}</a>")));
            assert!(html.contains("aria-label=\"Repository\" data-project-select"));
            assert!(html.contains(&format!("data-current=\"/projects/{project}/\"")));
            assert!(html.contains(&format!(
                "data-search-validation=\"/projects/{project}/collections/notes/search-validation\""
            )));
            assert!(html.contains(&format!(
                "data-url=\"/projects/{project}/collections/notes\""
            )));
        }
    }
    // Start a real draft in the second repository, then submit its form to both mounts.
    let start = HashMap::from([("csrf".into(), actor.csrf.clone())]);
    let html = body(
        call(
            &app,
            "POST",
            "/projects/second/collections/notes/records/one/edit-sessions",
            &cookie,
            &start,
            false,
        )
        .await,
    )
    .await;
    let id = html
        .split("data-draft-id=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let mut form = HashMap::new();
    lol_html::rewrite_str(
        &html,
        lol_html::RewriteStrSettings {
            element_content_handlers: vec![lol_html::element!("#record-form input[name]", |el| {
                let name = el.get_attribute("name").unwrap();
                let mut value = el.get_attribute("value").unwrap_or_default();
                if value == "Second" {
                    value = "Changed in second repository".into();
                }
                form.insert(name, value);
                Ok(())
            })],
            ..Default::default()
        },
    )
    .unwrap();
    let foreign = body(
        call(
            &app,
            "POST",
            &format!("/projects/first/edit-sessions/{id}/save"),
            &cookie,
            &form,
            true,
        )
        .await,
    )
    .await;
    assert!(foreign.contains("Draft expired"));
    assert_eq!(
        state.editor.get("notes", "one").unwrap().value["name"],
        "First"
    );
    let saved = body(
        call(
            &app,
            "POST",
            &format!("/projects/second/edit-sessions/{id}/save"),
            &cookie,
            &form,
            true,
        )
        .await,
    )
    .await;
    assert!(saved.contains("Saved to disk"), "{saved}");
    assert_eq!(
        other.editor.get("notes", "one").unwrap().value["name"],
        "Changed in second repository"
    );
    assert_eq!(
        state.editor.get("notes", "one").unwrap().value["name"],
        "First"
    );
    let exported = body(
        call(
            &app,
            "GET",
            "/projects/second/collections/notes/records/one/export",
            &cookie,
            &empty,
            false,
        )
        .await,
    )
    .await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&exported).unwrap()["name"],
        "Changed in second repository"
    );
}
