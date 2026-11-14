use std::path::{Path, PathBuf};

use super::*;

fn arguments() -> Arguments {
    Arguments {
        path: vec![PathBuf::from("/tmp/archive")],
        no_recurse: false,
        auto_index: true,
        index_alphabet: Some(Alphabet::Unicode),
        ..Arguments::default()
    }
}

fn paths() -> ResolvedPaths {
    ResolvedPaths {
        root: PathBuf::from("/tmp/archive"),
        collection_paths: vec![PathBuf::from("/tmp/archive")],
        schema_file: Some(PathBuf::from("/tmp/archive/schema.json")),
    }
}

#[test]
fn recipe_round_trips_paths_and_options() {
    let recipe = OpeningRecipe::from_opening(&arguments(), &paths());
    assert_eq!(recipe.version, OPENING_RECIPE_VERSION);
    assert_eq!(recipe.source_mode, RecipeSourceMode::Folder);
    assert_eq!(recipe.effective_alphabet(), Alphabet::Unicode);
    assert!(recipe.auto_index);
    assert!(recipe.recursive);
    let metadata = recipe.to_metadata();
    let decoded = OpeningRecipe::from_metadata(Some(&metadata))
        .unwrap()
        .unwrap();
    assert_eq!(decoded, recipe);
    let effective = decoded.apply(&Arguments::default()).unwrap();
    assert_eq!(effective.path, vec![PathBuf::from("/tmp/archive")]);
    assert_eq!(
        effective.schema,
        Some(PathBuf::from("/tmp/archive/schema.json"))
    );
    assert_eq!(effective.index_alphabet, Some(Alphabet::Unicode));
    assert!(effective.auto_index);
    assert!(!effective.no_recurse);
}

#[test]
fn omitted_alphabet_is_distinct_from_explicit_ascii() {
    let args = Arguments::default();
    assert_eq!(args.index_alphabet, None);
    assert_eq!(args.effective_alphabet(), Alphabet::Ascii);
    let explicit = Arguments {
        index_alphabet: Some(Alphabet::Ascii),
        ..Arguments::default()
    };
    assert_eq!(explicit.index_alphabet, Some(Alphabet::Ascii));
}

#[test]
fn unsupported_recipe_versions_and_missing_keys_are_reported() {
    assert!(OpeningRecipe::from_metadata(None).unwrap().is_none());
    assert!(
        OpeningRecipe::from_metadata(Some(&serde_json::json!({"other": 1})))
            .unwrap()
            .is_none()
    );
    let mut recipe = OpeningRecipe::from_opening(&arguments(), &paths());
    recipe.version = OPENING_RECIPE_VERSION + 1;
    let error = OpeningRecipe::from_metadata(Some(&recipe.to_metadata())).unwrap_err();
    assert!(error.contains("version"));
}

#[test]
fn native_path_hex_is_lossless() {
    let recipe = OpeningRecipe::from_opening(&arguments(), &paths());
    let root = path_from_hex(&recipe.root).unwrap();
    assert_eq!(root, Path::new("/tmp/archive"));
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_vec(vec![
            b'/', b't', b'm', b'p', b'/', 0xff, 0xfe,
        ]));
        assert_eq!(path_from_hex(&path_to_hex(&path)).unwrap(), path);
    }
    assert!(path_from_hex("abc").is_err());
    assert!(path_from_hex("zz").is_err());
}

#[test]
fn resolved_paths_are_confirmed_against_the_recipe() {
    let recipe = OpeningRecipe::from_opening(&arguments(), &paths());
    recipe.validate_paths(&paths()).unwrap();
    let mut wrong = paths();
    wrong.root = PathBuf::from("/tmp/other");
    assert!(recipe.validate_paths(&wrong).is_err());
    let mut wrong = paths();
    wrong.schema_file = None;
    assert!(recipe.validate_paths(&wrong).is_err());
}

#[test]
fn multiple_folder_and_repository_modes_are_recorded() {
    let args = Arguments {
        path: vec![PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two")],
        ..Arguments::default()
    };
    let paths = ResolvedPaths {
        root: PathBuf::from("/tmp"),
        collection_paths: vec![PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two")],
        schema_file: None,
    };
    let recipe = OpeningRecipe::from_opening(&args, &paths);
    assert_eq!(recipe.source_mode, RecipeSourceMode::Folders);
    let repo_args = Arguments {
        repo: Some(PathBuf::from("/tmp/repo")),
        ..Arguments::default()
    };
    let repo_paths = ResolvedPaths {
        root: PathBuf::from("/tmp/repo"),
        collection_paths: Vec::new(),
        schema_file: None,
    };
    let recipe = OpeningRecipe::from_opening(&repo_args, &repo_paths);
    assert_eq!(recipe.source_mode, RecipeSourceMode::Repository);
    let effective = recipe.apply(&Arguments::default()).unwrap();
    assert_eq!(effective.repo, Some(PathBuf::from("/tmp/repo")));
    assert!(effective.path.is_empty());
}
