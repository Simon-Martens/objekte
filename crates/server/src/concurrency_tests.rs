//! Authenticated HTTP races: separate users, drafts and CSRF tokens.
use super::*;

async fn second_user(state: &AppState) -> (String, auth::Actor) {
    let email = "second-editor@example.test";
    let token = state.auth.invite(email, "editor").await.unwrap();
    state
        .auth
        .redeem_invitation(&token, email, "Second editor", "second-password-123".into())
        .await
        .unwrap();
    let token = state
        .auth
        .login(email, "second-password-123".into())
        .await
        .unwrap();
    let cookie = format!("objekte_session={token}");
    let mut headers = HeaderMap::new();
    headers.insert("cookie", cookie.parse().unwrap());
    (cookie, state.auth.actor(&headers).await.unwrap())
}
fn open_draft(state: &AppState, actor: &auth::Actor, id: &str) -> sessions::EditSession {
    state
        .sessions
        .lock()
        .unwrap()
        .start(&state.editor, actor, "notes", Some(id))
        .unwrap()
}
fn changed_form(
    session: &sessions::EditSession,
    actor: &auth::Actor,
    name: &str,
    count: i64,
) -> HashMap<String, String> {
    let mut input = form(session, actor);
    for (key, value) in [("name", name.to_owned()), ("count", count.to_string())] {
        let field = session.draft.fields.iter().find(|f| f.key == key).unwrap();
        input.insert(format!("value.{}", field.id), value);
    }
    input
}
type Post<'a> = (&'a str, &'a str, &'a HashMap<String, String>);
async fn race(
    state: Arc<AppState>,
    first: Post<'_>,
    second: Post<'_>,
) -> ((StatusCode, String), (StatusCode, String)) {
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let spawn_request = |post: Post<'_>| {
        let state = state.clone();
        let barrier = barrier.clone();
        let (url, cookie, form) = (post.0.to_owned(), post.1.to_owned(), post.2.clone());
        tokio::spawn(async move {
            barrier.wait().await;
            request(state, "POST", &url, &cookie, &form, true).await
        })
    };
    let first = spawn_request(first);
    let second = spawn_request(second);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let (first, second) = tokio::join!(first, second);
        (
            first.expect("first request panicked"),
            second.expect("second request panicked"),
        )
    })
    .await
    .expect("concurrent requests must finish without deadlocking")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_users_saving_same_revision_have_one_winner_and_preserve_losing_draft() {
    let (_temp, state, cookie_a, actor_a) = fixture().await;
    let (cookie_b, actor_b) = second_user(&state).await;
    assert_ne!(actor_a.id, actor_b.id);
    assert_ne!(actor_a.session, actor_b.session);
    assert_ne!(actor_a.csrf, actor_b.csrf);
    let a = open_draft(&state, &actor_a, "one");
    let b = open_draft(&state, &actor_b, "one");
    assert_eq!(a.revision, b.revision);
    let input_a = changed_form(&a, &actor_a, "Alice's complete edit", 11);
    let input_b = changed_form(&b, &actor_b, "Bob's complete edit", 22);
    let url_a = format!("/edit-sessions/{}/save", a.id);
    let url_b = format!("/edit-sessions/{}/save", b.id);
    let (response_a, response_b) = race(
        state.clone(),
        (&url_a, &cookie_a, &input_a),
        (&url_b, &cookie_b, &input_b),
    )
    .await;
    // Save conflicts are rendered in the editor over a successful SSE transport.
    assert_eq!(response_a.0, StatusCode::OK);
    assert_eq!(response_b.0, StatusCode::OK);
    let saved = state.editor.get("notes", "one").unwrap();
    let current_a = state
        .sessions
        .lock()
        .unwrap()
        .get(&a.id, &actor_a)
        .unwrap()
        .clone();
    let current_b = state
        .sessions
        .lock()
        .unwrap()
        .get(&b.id, &actor_b)
        .unwrap()
        .clone();
    let a_won = saved.value["name"] == "Alice's complete edit";
    let (
        winner,
        loser,
        losing_response,
        losing_actor,
        losing_cookie,
        losing_url,
        losing_name,
        losing_count,
    ) = if a_won {
        (
            &current_a,
            &current_b,
            &response_b.1,
            &actor_b,
            &cookie_b,
            &url_b,
            "Bob's complete edit",
            22,
        )
    } else {
        (
            &current_b,
            &current_a,
            &response_a.1,
            &actor_a,
            &cookie_a,
            &url_a,
            "Alice's complete edit",
            11,
        )
    };
    assert_eq!(
        saved.value["name"],
        if a_won {
            "Alice's complete edit"
        } else {
            "Bob's complete edit"
        }
    );
    assert_eq!(saved.value["count"], if a_won { 11 } else { 22 });
    assert_eq!(winner.value().unwrap(), saved.value);
    assert_eq!(winner.revision.as_ref(), Some(&saved.revision));
    assert!(!winner.dirty());
    assert_eq!(loser.revision, a.revision);
    assert!(loser.dirty() && loser.history.can_undo());
    assert_eq!(loser.value().unwrap()["name"], losing_name);
    assert_eq!(loser.value().unwrap()["count"], losing_count);
    assert!(losing_response.contains("record changed on disk"));
    assert!(losing_response.contains("<template data-notification=\"error\">"));
    assert_eq!(
        [&response_a.1, &response_b.1]
            .iter()
            .filter(|html| html.contains("Saved to disk."))
            .count(),
        1
    );
    // Retrying cannot overwrite the winner without explicitly resolving the stale revision.
    let (_, retry) = request(
        state.clone(),
        "POST",
        losing_url,
        losing_cookie,
        &form(loser, losing_actor),
        true,
    )
    .await;
    assert!(retry.contains("record changed on disk"));
    assert_eq!(
        state.editor.get("notes", "one").unwrap().revision,
        saved.revision
    );
    // Neither authenticated user can read the other user's private draft.
    for (id, cookie) in [(&a.id, &cookie_b), (&b.id, &cookie_a)] {
        assert_eq!(
            request(
                state.clone(),
                "GET",
                &format!("/edit-sessions/{id}"),
                cookie,
                &HashMap::new(),
                false
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_users_saving_different_records_both_succeed() {
    let (_temp, state, cookie_a, actor_a) = fixture().await;
    let (cookie_b, actor_b) = second_user(&state).await;
    state
        .editor
        .save("notes", "two", json!({"name":"Second","count":2}), None)
        .unwrap();
    let a = open_draft(&state, &actor_a, "one");
    let b = open_draft(&state, &actor_b, "two");
    let input_a = changed_form(&a, &actor_a, "Alice's record", 11);
    let input_b = changed_form(&b, &actor_b, "Bob's record", 22);
    let (response_a, response_b) = race(
        state.clone(),
        (
            &format!("/edit-sessions/{}/save", a.id),
            &cookie_a,
            &input_a,
        ),
        (
            &format!("/edit-sessions/{}/save", b.id),
            &cookie_b,
            &input_b,
        ),
    )
    .await;
    for (response, session, actor, name, count) in [
        (&response_a, &a, &actor_a, "Alice's record", 11),
        (&response_b, &b, &actor_b, "Bob's record", 22),
    ] {
        assert_eq!(response.0, StatusCode::OK);
        assert!(response.1.contains("Saved to disk."));
        assert!(
            response
                .1
                .contains("<template data-notification=\"success\">")
        );
        let saved = state.editor.get("notes", &session.record_id).unwrap();
        assert_eq!(saved.value["name"], name);
        assert_eq!(saved.value["count"], count);
        let draft = state
            .sessions
            .lock()
            .unwrap()
            .get(&session.id, actor)
            .unwrap()
            .clone();
        assert!(!draft.dirty());
        assert_eq!(draft.revision.as_ref(), Some(&saved.revision));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn save_versus_delete_has_one_winner_and_never_resurrects_a_record() {
    let (_temp, state, cookie_a, actor_a) = fixture().await;
    let (cookie_b, actor_b) = second_user(&state).await;
    // Exercise the live race plus both deterministic operation orders.
    for (id, order) in [("racing", 0), ("save-first", 1), ("delete-first", 2)] {
        state
            .editor
            .save("notes", id, json!({"name":"Original","count":1}), None)
            .unwrap();
        let draft = open_draft(&state, &actor_a, id);
        let save_form = changed_form(&draft, &actor_a, "Unsaved work", 42);
        let delete_form = HashMap::from([
            ("csrf".into(), actor_b.csrf.clone()),
            ("revision".into(), draft.revision.clone().unwrap()),
        ]);
        let save_url = format!("/edit-sessions/{}/save", draft.id);
        let delete_url = format!("/collections/notes/records/{id}/delete");
        let (save, delete) = match order {
            1 => {
                let save = request(
                    state.clone(),
                    "POST",
                    &save_url,
                    &cookie_a,
                    &save_form,
                    true,
                )
                .await;
                let delete = request(
                    state.clone(),
                    "POST",
                    &delete_url,
                    &cookie_b,
                    &delete_form,
                    true,
                )
                .await;
                (save, delete)
            }
            2 => {
                let delete = request(
                    state.clone(),
                    "POST",
                    &delete_url,
                    &cookie_b,
                    &delete_form,
                    true,
                )
                .await;
                let save = request(
                    state.clone(),
                    "POST",
                    &save_url,
                    &cookie_a,
                    &save_form,
                    true,
                )
                .await;
                (save, delete)
            }
            _ => {
                race(
                    state.clone(),
                    (&save_url, &cookie_a, &save_form),
                    (&delete_url, &cookie_b, &delete_form),
                )
                .await
            }
        };
        let save_won = save.1.contains("Saved to disk.");
        let delete_won = delete.1.contains("Record deleted.");
        assert_ne!(
            save_won, delete_won,
            "exactly one mutation must succeed: save={save:?}, delete={delete:?}"
        );
        if order == 1 {
            assert!(save_won);
        }
        if order == 2 {
            assert!(delete_won);
        }
        let current = state
            .sessions
            .lock()
            .unwrap()
            .get(&draft.id, &actor_a)
            .unwrap()
            .clone();
        if save_won {
            assert_eq!(delete.0, StatusCode::CONFLICT);
            assert!(delete.1.contains("record changed on disk"));
            let saved = state.editor.get("notes", id).unwrap();
            assert_eq!(saved.value["name"], "Unsaved work");
            assert_eq!(saved.value["count"], 42);
            assert!(!current.dirty());
        } else {
            assert!(state.editor.get("notes", id).is_err());
            assert!(
                save.1.contains("not found") || save.1.contains("record was removed"),
                "{save:?}"
            );
            assert_eq!(current.revision, draft.revision);
            assert!(current.dirty());
            assert_eq!(current.value().unwrap()["name"], "Unsaved work");
            assert_eq!(current.value().unwrap()["count"], 42);
            request(
                state.clone(),
                "POST",
                &save_url,
                &cookie_a,
                &form(&current, &actor_a),
                true,
            )
            .await;
            assert!(
                state.editor.get("notes", id).is_err(),
                "a stale save must never recreate a deleted record"
            );
        }
    }
}
