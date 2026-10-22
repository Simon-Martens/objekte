use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
};
use structured_editor_server::{AppState, assets::Assets, error::WebError};
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "structured_editor_server=info".into()),
        )
        .init();
    if let Err(error) = run().await {
        eprintln!("{}", error.1);
        std::process::exit(1);
    }
}
async fn run() -> Result<(), WebError> {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = std::env::var_os("EDITOR_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("examples/lenz-archiv"));
    let directory = std::env::var_os("EDITOR_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join(".data"));
    let assets_root = std::env::var_os("EDITOR_ASSETS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/dist"));
    let admin = std::env::args().nth(1).as_deref() == Some("admin-create");
    let assets = if admin {
        Assets {
            root: assets_root,
            tags: String::new(),
        }
    } else {
        Assets::load(
            &assets_root,
            std::env::var("EDITOR_VITE_URL").ok().as_deref(),
        )?
    };
    let state = AppState::open(
        &root,
        &directory,
        assets,
        std::env::var("EDITOR_INSECURE_COOKIE").as_deref() == Ok("1"),
    )
    .await?;
    if admin {
        let interactive = io::stdin().is_terminal();
        fn line(prompt: &str, interactive: bool) -> Result<String, WebError> {
            if interactive {
                print!("{prompt}");
                io::stdout().flush().map_err(WebError::internal)?;
            }
            let mut value = String::new();
            io::stdin()
                .read_line(&mut value)
                .map_err(WebError::internal)?;
            Ok(value.trim_end_matches(['\r', '\n']).into())
        }
        let email = line("Email: ", interactive)?;
        let name = line("Display name: ", interactive)?;
        let password = if interactive {
            rpassword::prompt_password("Password: ").map_err(WebError::internal)?
        } else {
            line("", false)?
        };
        state.auth.create_admin(&email, &name, password).await?;
        println!("Administrator created.");
        return Ok(());
    }
    let bind = std::env::var("EDITOR_BIND").unwrap_or_else(|_| "127.0.0.1:4322".into());
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .map_err(WebError::internal)?;
    tracing::info!(%bind,"Objekte Rust server listening");
    axum::serve(listener, structured_editor_server::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(WebError::internal)
}
