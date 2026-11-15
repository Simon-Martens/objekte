use super::*;

fn root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "objekte-index-sync-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn identity(seed: u8) -> [u8; 32] {
    [seed; 32]
}

#[test]
fn memory_has_no_store_file() {
    let root = root("memory");
    assert_eq!(
        store_file(&IndexSync::Memory, &root, &identity(0)).unwrap(),
        None
    );
}

#[test]
fn repository_store_is_below_dot_objekte() {
    let root = root("repository");
    let file = store_file(&IndexSync::Repository, &root, &identity(0xab))
        .unwrap()
        .unwrap();
    assert_eq!(
        file,
        root.join(".objekte")
            .join(".index")
            .join("abababababababab.redb")
    );
}

#[test]
fn cache_and_custom_stores_use_the_generated_directory_name() {
    let root = root("cache");
    let custom = store_file(&IndexSync::Custom(root.join("custom")), &root, &identity(1))
        .unwrap()
        .unwrap();
    let relative = custom.strip_prefix(root.join("custom")).unwrap();
    assert!(crate::discovery::contains_generated_directory(relative));
    let components = relative.components().collect::<Vec<_>>();
    assert_eq!(components[0].as_os_str(), "objekte-indexes");
    let key = components[1].as_os_str().to_string_lossy().to_string();
    assert_eq!(key.len(), 16);
    assert!(key.chars().all(|character| character.is_ascii_hexdigit()));
}

#[test]
fn different_roots_and_configurations_get_different_files() {
    let first = root("different-a");
    let second = root("different-b");
    let a = store_file(&IndexSync::Repository, &first, &identity(1))
        .unwrap()
        .unwrap();
    let b = store_file(&IndexSync::Repository, &second, &identity(1))
        .unwrap()
        .unwrap();
    let c = store_file(&IndexSync::Repository, &first, &identity(2))
        .unwrap()
        .unwrap();
    assert_ne!(a, b);
    assert_ne!(a, c);
    assert_ne!(a.file_name(), c.file_name());
}

#[test]
fn relative_custom_directory_is_rejected() {
    assert!(store_file(
        &IndexSync::Custom(PathBuf::from("relative")),
        &root("relative"),
        &identity(0)
    )
    .is_err());
}

#[test]
fn cache_directory_resolution() {
    let previous_cache = std::env::var_os("OBJEKTE_CACHE_DIR");
    let previous_xdg = std::env::var_os("XDG_CACHE_HOME");
    let previous_home = std::env::var_os("HOME");
    let root = root("environment");
    std::fs::create_dir_all(&root).unwrap();

    unsafe { std::env::set_var("OBJEKTE_CACHE_DIR", root.join("absolute-cache")); }
    assert_eq!(system_cache_dir().unwrap(), root.join("absolute-cache"));

    unsafe { std::env::set_var("OBJEKTE_CACHE_DIR", "relative-cache"); }
    unsafe { std::env::set_var("XDG_CACHE_HOME", root.join("xdg")); }
    assert_eq!(system_cache_dir().unwrap(), root.join("xdg"));

    unsafe { std::env::set_var("XDG_CACHE_HOME", "relative-xdg"); }
    unsafe { std::env::set_var("HOME", root.join("home")); }
    assert_eq!(
        system_cache_dir().unwrap(),
        root.join("home").join(".cache")
    );

    unsafe { std::env::remove_var("OBJEKTE_CACHE_DIR"); }
    unsafe { std::env::remove_var("XDG_CACHE_HOME"); }
    unsafe { std::env::remove_var("HOME"); }
    assert_eq!(
        system_cache_dir(),
        Err("no cache directory is available".into())
    );

    match previous_cache {
        Some(value) => unsafe { std::env::set_var("OBJEKTE_CACHE_DIR", value) },
        None => unsafe { std::env::remove_var("OBJEKTE_CACHE_DIR") },
    }
    match previous_xdg {
        Some(value) => unsafe { std::env::set_var("XDG_CACHE_HOME", value) },
        None => unsafe { std::env::remove_var("XDG_CACHE_HOME") },
    }
    match previous_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    std::fs::remove_dir_all(root).unwrap();
}
