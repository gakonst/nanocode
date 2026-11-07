use crate::browsers::browser_dirs;
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub fn manifest_path(root: &Path, name: &str) -> PathBuf {
    root.join("NativeMessagingHosts")
        .join(format!("{name}.json"))
}

pub fn extension_origin(extension: &str) -> String {
    format!("chrome-extension://{extension}/")
}

#[derive(Clone, Debug)]
pub struct NativeHost {
    name: String,
    description: String,
    path: PathBuf,
    origins: Vec<String>,
}

impl NativeHost {
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            path: path.into(),
            origins: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn extension(mut self, extension: &str) -> Self {
        self.origins.push(extension_origin(extension));
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn manifest(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "path": self.path,
            "type": "stdio",
            "allowed_origins": self.origins,
        })
    }

    pub fn owns(&self, manifest: &Path) -> bool {
        fs::read(manifest)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|value| value["path"].as_str() == self.path.to_str())
    }

    pub fn register(&self, base: &Path) -> io::Result<Vec<&'static str>> {
        let manifest = serde_json::to_vec_pretty(&self.manifest())?;
        let mut registered = Vec::new();
        let mut written = Vec::new();
        for (browser, root) in browser_dirs(base) {
            let path = manifest_path(&root, &self.name);
            if !root.is_dir() || written.contains(&path) {
                continue;
            }
            if let Some(hosts) = path.parent() {
                fs::create_dir_all(hosts)?;
            }
            fs::write(&path, &manifest)?;
            written.push(path);
            registered.push(browser);
        }
        Ok(registered)
    }

    pub fn unregister(&self, base: &Path) -> Vec<&'static str> {
        let mut removed = Vec::new();
        for (browser, root) in browser_dirs(base) {
            let path = manifest_path(&root, &self.name);
            if self.owns(&path) && fs::remove_file(path).is_ok() {
                removed.push(browser);
            }
        }
        removed
    }
}
