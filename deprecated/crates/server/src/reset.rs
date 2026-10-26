use std::{fs, io, path::Path};
use structured_editor_server::error::WebError;

// Keep all Rust server state cleanup here as new on-disk caches are introduced.
pub fn run(project: &Path, state: &Path) -> Result<(), WebError> {
    let project = project.canonicalize().map_err(WebError::internal)?;
    let state = match state.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            println!("Rust server state is already empty.");
            return Ok(());
        }
        Err(error) => return Err(WebError::internal(error)),
    };
    if state.starts_with(&project) || project.starts_with(&state) {
        return Err(WebError::bad(
            "Administrative storage and project content must be separate.",
        ));
    }

    for name in [
        "server.sqlite-wal",
        "server.sqlite-shm",
        "server.sqlite-journal",
        "server.sqlite",
    ] {
        let path = state.join(name);
        match fs::remove_file(&path) {
            Ok(()) => println!("Removed {}", path.display()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(WebError::internal(format!(
                    "Could not remove {}: {error}",
                    path.display()
                )));
            }
        }
    }
    println!(
        "Rust server state reset. Run `cargo run -p structured-editor-server -- admin-create` to create a new administrator."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_removes_only_rust_server_database_files() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let state = temp.path().join("state");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&state).unwrap();
        for name in [
            "server.sqlite",
            "server.sqlite-wal",
            "server.sqlite-shm",
            "server.sqlite-journal",
            "editor.sqlite",
        ] {
            fs::write(state.join(name), b"test").unwrap();
        }
        fs::write(project.join("record.json"), b"{}").unwrap();

        run(&project, &state).unwrap();
        for name in [
            "server.sqlite",
            "server.sqlite-wal",
            "server.sqlite-shm",
            "server.sqlite-journal",
        ] {
            assert!(!state.join(name).exists());
        }
        assert!(state.join("editor.sqlite").exists());
        assert!(project.join("record.json").exists());
        run(&project, &state).unwrap();
    }

    #[test]
    fn reset_refuses_project_directory() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let database = project.join("server.sqlite");
        fs::write(&database, b"test").unwrap();

        assert!(run(&project, &project).is_err());
        assert!(database.exists());

        let state_inside_project = project.join("state");
        fs::create_dir(&state_inside_project).unwrap();
        let nested_database = state_inside_project.join("server.sqlite");
        fs::write(&nested_database, b"test").unwrap();
        assert!(run(&project, &state_inside_project).is_err());
        assert!(nested_database.exists());
    }
}
