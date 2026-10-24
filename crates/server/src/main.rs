use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
};
use structured_editor_server::{AppState, assets::Assets, error::WebError};

mod reset;

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
    let args: Vec<_> = std::env::args().skip(1).collect();
    let usage = "Usage: structured-editor-server [--dev] [admin-create | reset]";
    if args
        .iter()
        .any(|arg| !matches!(arg.as_str(), "admin-create" | "reset" | "--dev"))
        || args.iter().filter(|arg| arg.as_str() == "--dev").count() > 1
        || args
            .iter()
            .filter(|arg| matches!(arg.as_str(), "admin-create" | "reset"))
            .count()
            > 1
    {
        return Err(WebError::bad(usage));
    }
    if args.iter().any(|arg| arg == "reset") {
        reset::run(&root, &directory)?;
        return Ok(());
    }
    let admin = args.iter().any(|arg| arg == "admin-create");
    let dev =
        args.iter().any(|arg| arg == "--dev") || std::env::var("EDITOR_DEV").as_deref() == Ok("1");
    let vite_url = std::env::var("EDITOR_VITE_URL")
        .ok()
        .or_else(|| dev.then(|| "http://127.0.0.1:5173".into()));
    structured_editor_server::templates::init(dev)?;
    let assets = if admin {
        Assets {
            root: assets_root,
            tags: String::new(),
        }
    } else {
        Assets::load(&assets_root, vite_url.as_deref())?
    };
    let insecure = dev || std::env::var("EDITOR_INSECURE_COOKIE").as_deref() == Ok("1");
    let catalog = std::env::var_os("EDITOR_PROJECTS")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("EDITOR_ROOT")
                .is_none()
                .then(|| workspace.join("examples/projects.json"))
        });
    let (state, app) = if let Some(path) = catalog {
        let catalog = structured_editor_core::projects::Projects::load(&path)?;
        let server = structured_editor_server::projects::ProjectServer::open(
            catalog, &directory, assets, insecure,
        )
        .await?;
        (server.default_state(), server.router())
    } else {
        let state = AppState::open(&root, &directory, assets, insecure).await?;
        (state.clone(), structured_editor_server::router(state))
    };
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
    if dev {
        tracing::info!(vite = ?vite_url, "Development mode: live assets, templates read from disk, HTTP cookies");
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(WebError::internal)
}
