use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use axum::{
    Router,
    extract::Request,
    middleware::{Next, from_fn},
    response::Response,
};

/// Install once on the outer router, after selecting single- or multi-project mode.
pub fn development(app: Router, enabled: bool) -> Router {
    if enabled {
        app.layer(from_fn(log_request))
    } else {
        app
    }
}

async fn log_request(request: Request, next: Next) -> Response {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let request_id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let started = Instant::now();
    let method = request.method().clone();
    // Queries can contain invitation/reset tokens; bodies and headers can contain credentials.
    let path = request.uri().path().to_owned();
    tracing::info!(request_id, %method, %path, "HTTP request");

    let response = next.run(request).await;
    // Datastar receives error notices over HTTP 200, with the original status in this header.
    let error_status = response
        .headers()
        .get("x-editor-error-status")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u16>().ok());
    tracing::info!(
        request_id,
        %method,
        %path,
        status = response.status().as_u16(),
        error_status,
        elapsed_ms = format_args!("{:.3}", started.elapsed().as_secs_f64() * 1000.0),
        "HTTP response"
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
        time::Duration,
    };

    use axum::{body::Body, http::StatusCode, routing::post};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use tracing::instrument::WithSubscriber;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Capture {
        fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
            let writer = self.clone();
            tracing_subscriber::fmt()
                .without_time()
                .with_ansi(false)
                .with_target(false)
                .with_writer(move || writer.clone())
                .finish()
        }

        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    fn field<'a>(line: &'a str, name: &str) -> &'a str {
        line.split_whitespace()
            .find_map(|part| part.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("missing {name}: {line}"))
    }

    #[tokio::test]
    async fn logs_correlated_request_and_response_with_handler_time() {
        let capture = Capture::default();
        let app = development(
            Router::new().route(
                "/records",
                post(|| async {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    (
                        StatusCode::CREATED,
                        [("x-test", "preserved")],
                        "response body",
                    )
                }),
            ),
            true,
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/records?token=query-secret")
                    .header("authorization", "Bearer header-secret")
                    .header("cookie", "session=cookie-secret")
                    .body(Body::from("body-secret"))
                    .unwrap(),
            )
            .with_subscriber(capture.subscriber())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()["x-test"], "preserved");
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "response body"
        );
        let logs = capture.text();
        let lines: Vec<_> = logs.lines().collect();
        assert_eq!(lines.len(), 2, "{logs}");
        assert!(lines[0].contains("HTTP request"));
        assert!(lines[1].contains("HTTP response"));
        assert_eq!(field(lines[0], "request_id"), field(lines[1], "request_id"));
        for line in &lines {
            assert_eq!(field(line, "method"), "POST");
            assert_eq!(field(line, "path"), "/records");
        }
        assert_eq!(field(lines[1], "status"), "201");
        assert!(field(lines[1], "elapsed_ms").parse::<f64>().unwrap() >= 20.0);
        assert!(!logs.contains("secret"));
        assert!(!logs.contains("response body"));
    }

    #[tokio::test]
    async fn logs_fallbacks_and_datastar_error_status_without_changing_responses() {
        let capture = Capture::default();
        let app = development(
            Router::new().route(
                "/error",
                post(|| async {
                    (
                        [
                            ("x-editor-error-status", "422"),
                            ("content-type", "text/event-stream"),
                        ],
                        "event: datastar-patch-elements\ndata: elements <div>Error</div>\n\n",
                    )
                }),
            ),
            true,
        );
        let mut ids = Vec::new();
        for (path, status) in [
            ("/missing", StatusCode::NOT_FOUND),
            ("/error", StatusCode::OK),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .with_subscriber(capture.subscriber())
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            if path == "/error" {
                assert_eq!(response.headers()["x-editor-error-status"], "422");
                assert_eq!(response.headers()["content-type"], "text/event-stream");
            }
        }
        let logs = capture.text();
        let lines: Vec<_> = logs.lines().collect();
        assert_eq!(lines.len(), 4, "{logs}");
        for pair in lines.chunks_exact(2) {
            assert_eq!(field(pair[0], "request_id"), field(pair[1], "request_id"));
            ids.push(field(pair[0], "request_id"));
        }
        assert_ne!(ids[0], ids[1]);
        assert_eq!(field(lines[1], "status"), "404");
        assert_eq!(field(lines[3], "status"), "200");
        assert_eq!(field(lines[3], "error_status"), "422");
    }

    #[tokio::test]
    async fn production_has_no_request_logging() {
        let capture = Capture::default();
        let response = development(Router::new(), false)
            .oneshot(Request::new(Body::empty()))
            .with_subscriber(capture.subscriber())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(capture.text().is_empty());
    }
}
