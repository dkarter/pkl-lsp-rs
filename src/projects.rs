use crate::schema;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAX_PROJECT_BYTES: u64 = 1024 * 1024;

#[derive(Default)]
pub struct Projects {
    roots: Vec<PathBuf>,
    invalid_workspace: bool,
    synced: HashMap<PathBuf, HashMap<String, PathBuf>>,
}

impl Projects {
    pub fn initialize(&mut self, params: &Value) {
        *self = Self::default();
        let uris: Vec<_> = if let Some(folders) = params["workspaceFolders"].as_array() {
            folders.iter().map(|folder| &folder["uri"]).collect()
        } else {
            params
                .get("rootUri")
                .filter(|uri| !uri.is_null())
                .into_iter()
                .collect()
        };
        if uris.len() > 32 {
            self.invalid_workspace = true;
            return;
        }
        for uri in uris {
            let path = uri
                .as_str()
                .and_then(|uri| url::Url::parse(uri).ok())
                .and_then(|uri| uri.to_file_path().ok())
                .and_then(|path| path.canonicalize().ok());
            if let Some(path) = path.filter(|path| path.is_dir()) {
                self.roots.push(path);
            } else {
                self.invalid_workspace = true;
            }
        }
    }

    pub fn sync(&mut self) -> Result<(), &'static str> {
        // Failure invalidates old mappings; do not serve stale dependencies.
        self.synced.clear();
        if self.invalid_workspace {
            return Err("Project sync requires at most 32 existing local file workspace folders");
        }
        let mut synced = HashMap::new();
        for root in &self.roots {
            let project = root.join("PklProject");
            if !project
                .try_exists()
                .map_err(|_| "Could not inspect PklProject")?
            {
                continue;
            }
            if root
                .join("PklProject.deps.json")
                .try_exists()
                .map_err(|_| "Could not inspect project lockfile")?
            {
                return Err("Project lockfiles are not supported by local-only sync");
            }
            let source = read_bounded(&project, MAX_PROJECT_BYTES)
                .ok_or("Could not read PklProject within the 1 MiB limit")?;
            let declared = schema::local_project_dependencies(&source)
                .ok_or("Unsupported PklProject: only amends pkl:Project and literal local dependency imports are supported (up to 64)")?;
            let mut dependencies = HashMap::new();
            for (alias, relative) in declared {
                let path = Path::new(&relative);
                if path.is_absolute()
                    || relative.contains([':', '%', '?', '#', '\\'])
                    || relative.chars().any(char::is_control)
                    || path.file_name().is_none_or(|name| name != "PklProject")
                {
                    return Err("Local dependencies must import a relative PklProject path");
                }
                let target = root
                    .join(path)
                    .canonicalize()
                    .map_err(|_| "Local dependency PklProject does not exist")?;
                if !target.is_file() {
                    return Err("Local dependency PklProject is not a file");
                }
                dependencies.insert(
                    alias,
                    target
                        .parent()
                        .ok_or("Invalid dependency directory")?
                        .to_owned(),
                );
            }
            synced.insert(root.clone(), dependencies);
        }
        self.synced = synced;
        Ok(())
    }

    pub fn resolve(&self, uri: &str, import: &str) -> Option<url::Url> {
        let (alias, member) = import.strip_prefix('@')?.split_once('/')?;
        if !schema::safe_member(member) {
            return None;
        }
        let path = url::Url::parse(uri).ok()?.to_file_path().ok()?;
        let directory = path.parent()?.canonicalize().ok()?;
        // An unsynced nested project is a boundary, not an alias fallback.
        let root = directory
            .ancestors()
            .find(|directory| directory.join("PklProject").exists())?;
        let dependency = self.synced.get(root)?.get(alias)?;
        let candidate = dependency.join(member);
        // Check symlinks via the file or its parent for an open, not-yet-saved file.
        let canonical = candidate.canonicalize().ok().or_else(|| {
            Some(
                candidate
                    .parent()?
                    .canonicalize()
                    .ok()?
                    .join(candidate.file_name()?),
            )
        })?;
        if !canonical.starts_with(dependency) {
            return None;
        }
        url::Url::from_file_path(candidate).ok()
    }
}

pub fn read_bounded(path: &Path, limit: u64) -> Option<String> {
    // Avoid opening devices/FIFOs, which can block before descriptor inspection.
    // This is not a sandbox against concurrent filesystem replacement.
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    schema::read_text(file, limit)
}
