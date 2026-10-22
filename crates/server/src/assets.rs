use crate::{
    error::{Result, WebError},
    views::escape,
};
use std::path::{Path, PathBuf};
#[derive(Clone)]
pub struct Assets {
    pub root: PathBuf,
    pub tags: String,
}
impl Assets {
    pub fn load(root: &Path, dev: Option<&str>) -> Result<Self> {
        if let Some(dev) = dev {
            let origin: axum::http::Uri = dev
                .parse()
                .map_err(|_| WebError::bad("Invalid Vite dev URL."))?;
            if origin.scheme_str() != Some("http")
                || !matches!(origin.host(), Some("127.0.0.1" | "localhost"))
                || origin.port_u16().is_none()
                || origin.path() != "/"
                || origin.query().is_some()
                || dev.contains('@')
            {
                return Err(WebError::bad(
                    "Vite dev URL must be a local HTTP origin with a port, e.g. http://127.0.0.1:5173.",
                ));
            }
            return Ok(Self {
                root: root.into(),
                tags: format!(
                    "<script type=\"module\" src=\"{}/assets/@vite/client\"></script><script type=\"module\" src=\"{}/assets/src/main.ts\"></script>",
                    escape(dev.trim_end_matches('/')),
                    escape(dev.trim_end_matches('/'))
                ),
            });
        }
        let bytes = std::fs::read(root.join(".vite/manifest.json")).map_err(|_| {
            WebError::bad(
                "Build the server assets first: npm ci && npm run build in crates/server/assets",
            )
        })?;
        let manifest: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(WebError::internal)?;
        let entry = &manifest["src/main.ts"];
        let file = entry["file"]
            .as_str()
            .ok_or_else(|| WebError::bad("Missing Vite entry"))?;
        let mut tags = String::new();
        fn css(
            manifest: &serde_json::Value,
            entry: &serde_json::Value,
            tags: &mut String,
            seen: &mut std::collections::HashSet<String>,
        ) {
            if let Some(files) = entry["css"].as_array() {
                for file in files.iter().filter_map(|v| v.as_str()) {
                    if seen.insert(file.into()) {
                        tags.push_str(&format!(
                            "<link rel=\"stylesheet\" href=\"/assets/{}\">",
                            escape(file)
                        ));
                    }
                }
            }
            if let Some(imports) = entry["imports"].as_array() {
                for key in imports.iter().filter_map(|v| v.as_str()) {
                    if seen.insert(key.into()) {
                        css(manifest, &manifest[key], tags, seen);
                    }
                }
            }
        }
        css(
            &manifest,
            entry,
            &mut tags,
            &mut std::collections::HashSet::new(),
        );
        tags.push_str(&format!(
            "<script type=\"module\" src=\"/assets/{}\"></script>",
            escape(file)
        ));
        Ok(Self {
            root: root.into(),
            tags,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_assets_do_not_require_a_production_build() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("missing-dist");
        let assets = Assets::load(&root, Some("http://127.0.0.1:5173/")).unwrap();
        assert!(
            assets
                .tags
                .contains("http://127.0.0.1:5173/assets/@vite/client")
        );
        assert!(
            assets
                .tags
                .contains("http://127.0.0.1:5173/assets/src/main.ts")
        );
        assert!(!assets.tags.contains("manifest"));
        assert!(Assets::load(&root, None).is_err());
        assert!(Assets::load(&root, Some("http://localhost:5174")).is_ok());
        for invalid in [
            "http://example.com:5173",
            "http://127.0.0.1:5173@evil.example",
            "http://localhost:5173/assets",
            "http://localhost:5173?foo=bar",
            "http://localhost:invalid",
        ] {
            assert!(Assets::load(&root, Some(invalid)).is_err(), "{invalid}");
        }
    }
}
