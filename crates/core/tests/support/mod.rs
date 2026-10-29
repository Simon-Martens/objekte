#![allow(dead_code)]
use objekte_core::{CacheMode, Options, Repository, ViewRequest, ViewRow};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};


// INFO: Temporary source repository and application data paths for tests.
pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub root: PathBuf,
    pub data: PathBuf,
}


pub fn copy(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for e in fs::read_dir(source).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_dir() {
            copy(&e.path(), &target.join(e.file_name()))
        } else {
            fs::copy(e.path(), target.join(e.file_name())).unwrap();
        }
    }
}


impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/minimal"),
            &root,
        );
        let data = temp.path().join("app");
        Self { temp, root, data }
    }

    pub fn options(&self, mode: CacheMode) -> Options {
        let mut o = Options::new(&self.data);
        o.cache = mode;
        // Reconciliation tests inspect ingestion independently of background scheduling.
        o.validation = objekte_core::ValidationMode::Manual;
        o
    }

    pub fn repo(&self, mode: CacheMode) -> Repository {
        Repository::load_repository(&self.root, self.options(mode)).unwrap()
    }

    pub fn read(&self, path: &str) -> Value {
        serde_json::from_slice(&fs::read(self.root.join(path)).unwrap()).unwrap()
    }

    pub fn write(&self, path: &str, value: &Value) {
        fs::write(self.root.join(path), serde_json::to_vec(value).unwrap()).unwrap()
    }

    pub fn edit(&self, path: &str, f: impl FnOnce(&mut Value)) {
        let mut v = self.read(path);
        f(&mut v);
        self.write(path, &v)
    }
}


pub fn all() -> ViewRequest {
    ViewRequest {
        limit: Some(10000),
        count: true,
        ..Default::default()
    }
}

pub fn ids(rows: &[ViewRow]) -> Vec<&str> {
    rows.iter().map(|r| r.id.as_str()).collect()
}

/// Physical raw-record table; no extracted field columns are stored here.
pub fn record_sql(collection: &str, sql: &str) -> String {
    use sha2::{Digest, Sha256};
    sql.replace(
        "records",
        &format!("records_{:x}", Sha256::digest(collection.as_bytes())),
    )
}
