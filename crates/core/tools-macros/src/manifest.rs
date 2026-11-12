use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    time::SystemTime,
};

use crate::toml::{self, Table, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FoundCrate {
    Itself,
    Name(String),
}

struct Cached {
    manifest_directory: PathBuf,
    manifest_modified: SystemTime,
    workspace_path: PathBuf,
    workspace_modified: SystemTime,
    names: BTreeMap<String, FoundCrate>,
}

pub(crate) fn crate_name(package: &str) -> Option<FoundCrate> {
    static CACHE: OnceLock<Option<Cached>> = OnceLock::new();
    let manifest_directory = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR")?);
    let manifest_path = manifest_directory.join("Cargo.toml");
    let manifest_modified = modified(&manifest_path)?;
    if let Some(Some(cached)) = CACHE.get()
        && cached.manifest_directory == manifest_directory
    {
        let workspace_modified = modified(&cached.workspace_path)?;
        if cached.manifest_modified == manifest_modified
            && cached.workspace_modified == workspace_modified
        {
            return cached.names.get(package).cloned();
        }
        let names = crate_names(&manifest_path, &cached.workspace_path)?;
        return names.get(package).cloned();
    }
    let workspace_path =
        workspace_manifest_path(&manifest_path)?.unwrap_or_else(|| manifest_path.clone());
    let workspace_modified = modified(&workspace_path)?;
    let names = crate_names(&manifest_path, &workspace_path)?;
    let found = names.get(package).cloned();
    let _ = CACHE.set(Some(Cached {
        manifest_directory,
        manifest_modified,
        workspace_path,
        workspace_modified,
        names,
    }));
    found
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn workspace_manifest_path(manifest_path: &Path) -> Option<Option<PathBuf>> {
    let Some(cargo) = env::var_os("CARGO") else {
        return Some(None);
    };
    let output = Command::new(cargo)
        .arg("locate-project")
        .args(["--workspace", "--message-format=plain"])
        .arg(format!("--manifest-path={}", manifest_path.display()))
        .output()
        .ok()?;
    let stdout = String::from_utf8(output.stdout).ok()?;
    let path = stdout.trim();
    Some((!path.is_empty()).then(|| PathBuf::from(path)))
}

fn read_manifest(path: &Path) -> Option<Table> {
    toml::parse(&fs::read_to_string(path).ok()?)
}

fn crate_names(
    manifest_path: &Path,
    workspace_path: &Path,
) -> Option<BTreeMap<String, FoundCrate>> {
    let manifest = read_manifest(manifest_path)?;
    let workspace_dependencies = if manifest_path == workspace_path {
        workspace_dependencies(&manifest)
    } else {
        workspace_dependencies(&read_manifest(workspace_path)?)
    };
    Some(extract_crate_names(&manifest, &workspace_dependencies))
}

fn package_of<'a>(name: &'a str, value: &'a Value) -> &'a str {
    value
        .as_table()
        .and_then(|table| table.get("package"))
        .and_then(Value::as_str)
        .unwrap_or(name)
}

fn workspace_dependencies(manifest: &Table) -> BTreeMap<String, String> {
    manifest
        .get("workspace")
        .and_then(Value::as_table)
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(Value::as_table)
        .into_iter()
        .flat_map(Table::iter)
        .map(|(name, value)| (name.to_owned(), package_of(name, value).to_owned()))
        .collect()
}

fn dependency_tables(table: &Table) -> impl Iterator<Item = &Table> {
    table
        .get("dependencies")
        .into_iter()
        .chain(table.get("dev-dependencies"))
        .filter_map(Value::as_table)
}

fn extract_crate_names(
    manifest: &Table,
    workspace_dependencies: &BTreeMap<String, String>,
) -> BTreeMap<String, FoundCrate> {
    let package_name = manifest
        .get("package")
        .and_then(Value::as_table)
        .and_then(|package| package.get("name"))
        .and_then(Value::as_str);
    let mut names = BTreeMap::new();
    if let Some(name) = package_name {
        let found = match env::var_os("CARGO_TARGET_TMPDIR") {
            None => FoundCrate::Itself,
            Some(_) => FoundCrate::Name(name.replace('-', "_")),
        };
        names.insert(name.to_owned(), found);
    }
    let targets = manifest
        .get("target")
        .and_then(Value::as_table)
        .into_iter()
        .flat_map(Table::iter)
        .filter_map(|(_, value)| value.as_table())
        .flat_map(dependency_tables);
    for table in dependency_tables(manifest).chain(targets) {
        for (name, value) in table.iter() {
            let package = package_of(name, value);
            if package_name == Some(package) {
                continue;
            }
            let inherited = value
                .as_table()
                .and_then(|table| table.get("workspace"))
                .and_then(Value::as_bool)
                .unwrap_or_default();
            let package = if inherited {
                workspace_dependencies
                    .get(package)
                    .map_or(package, String::as_str)
            } else {
                package
            };
            names.insert(package.to_owned(), FoundCrate::Name(name.replace('-', "_")));
        }
    }
    names
}
