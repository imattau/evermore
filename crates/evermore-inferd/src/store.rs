//! Model-store scanning and manifest validation.
//!
//! Layout, one directory per installed model version:
//!
//! ```text
//! <root>/<package>-<version>/
//!   manifest.json   ModelManifest
//!   base.gguf
//!   adapter.gguf    optional LoRA
//! ```
//!
//! The scanner trusts neither directory names nor manifest paths: it requires
//! a validated manifest and plain artifact filenames that stay inside the
//! model directory. Content hashes are verified by the signed-package
//! importer, not re-hashed on every daemon start.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use evermore_common::ai::{self, ModelManifest};

#[derive(Debug, Clone)]
pub struct InstalledModel {
    pub manifest: ModelManifest,
    pub base_path: PathBuf,
    pub adapter_path: Option<PathBuf>,
}

impl InstalledModel {
    pub fn name(&self) -> String {
        self.manifest.name()
    }

    pub fn size_bytes(&self) -> Option<u64> {
        let base = std::fs::metadata(&self.base_path).ok()?.len();
        let adapter = self
            .adapter_path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        Some(base + adapter)
    }

    pub fn modify_time(&self) -> Option<String> {
        let modified = std::fs::metadata(&self.base_path).ok()?.modified().ok()?;
        let seconds = modified.duration_since(UNIX_EPOCH).ok()?.as_secs();
        Some(seconds.to_string())
    }

    pub fn to_public_model(&self) -> ai::Model {
        ai::Model {
            name: self.name(),
            digest: Some(self.manifest.base.sha256.clone()),
            size_bytes: self.size_bytes(),
            modified_at: self.modify_time(),
            quant: (!self.manifest.quant.is_empty()).then(|| self.manifest.quant.clone()),
            context_size: Some(self.manifest.context_size),
            adapter: self
                .manifest
                .adapter
                .as_ref()
                .map(|adapter| adapter.file.clone()),
            capabilities: self.manifest.capabilities.clone(),
            operation_registry_version: self.manifest.operation_registry_version.clone(),
            loaded: false,
        }
    }
}

/// Scans roots in priority order. A model name found in an earlier root wins,
/// so the user store can shadow a system model. Invalid manifests are skipped
/// with a warning rather than taking the whole daemon down.
pub fn scan(roots: &[PathBuf]) -> Vec<InstalledModel> {
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for root in roots {
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                eprintln!(
                    "evermore-inferd: cannot read model root {}: {error}",
                    root.display()
                );
                continue;
            }
        };
        for entry in entries.flatten() {
            let directory = entry.path();
            if !directory.is_dir() {
                continue;
            }
            match load(&directory) {
                Ok(Some(model)) => {
                    if seen.insert(model.name()) {
                        models.push(model);
                    }
                }
                Ok(None) => {}
                Err(error) => eprintln!(
                    "evermore-inferd: ignoring model in {}: {error}",
                    directory.display()
                ),
            }
        }
    }
    models.sort_by_key(|model| model.name());
    models
}

pub fn find(roots: &[PathBuf], name: &str) -> Result<InstalledModel, String> {
    scan(roots)
        .into_iter()
        .find(|model| model.name() == name)
        .ok_or_else(|| {
            format!("model '{name}' is not installed; install a signed npack model package first")
        })
}

fn load(directory: &Path) -> Result<Option<InstalledModel>, String> {
    let manifest_path = directory.join("manifest.json");
    let bytes = match std::fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read manifest: {error}")),
    };
    let manifest: ModelManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid model manifest: {error}"))?;
    manifest.validate()?;

    let base_path = directory.join(&manifest.base.file);
    let base_path = canonical_inside(directory, &base_path)?;
    if !base_path.is_file() {
        return Err(format!("base model {} is missing", manifest.base.file));
    }
    let adapter_path = match &manifest.adapter {
        Some(adapter) => {
            let path = canonical_inside(directory, &directory.join(&adapter.file))?;
            if !path.is_file() {
                return Err(format!("model adapter {} is missing", adapter.file));
            }
            Some(path)
        }
        None => None,
    };
    Ok(Some(InstalledModel {
        manifest,
        base_path,
        adapter_path,
    }))
}

fn canonical_inside(directory: &Path, path: &Path) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| format!("model artifact {} is unavailable: {error}", path.display()))?;
    let root = std::fs::canonicalize(directory)
        .map_err(|error| format!("model directory is unavailable: {error}"))?;
    if !canonical.starts_with(&root) {
        return Err("model artifact resolves outside its model directory".into());
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use evermore_common::ai::{MODEL_MANIFEST_VERSION, ModelArtifact};

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "evermore-store-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_model(root: &Path, package: &str, version: &str) -> PathBuf {
        let directory = root.join(format!("{package}-{version}"));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("base.gguf"), b"gguf").unwrap();
        let manifest = ModelManifest {
            manifest_version: MODEL_MANIFEST_VERSION,
            package_id: package.into(),
            package_version: version.into(),
            description: String::new(),
            base: ModelArtifact {
                file: "base.gguf".into(),
                sha256: "a".repeat(64),
                size_bytes: None,
            },
            adapter: None,
            context_size: 2048,
            quant: "Q4_K_M".into(),
            capabilities: vec!["chat".into()],
            operation_registry_version: None,
            training: None,
            publisher: None,
        };
        std::fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        directory
    }

    #[test]
    fn scans_and_describes_models() {
        let root = temp_dir("scan");
        write_model(&root, "p7.model.tinyllama", "1.0.0");
        let models = scan(std::slice::from_ref(&root));
        assert_eq!(models.len(), 1);
        let public = models[0].to_public_model();
        assert_eq!(public.name, "evermore/p7.model.tinyllama:1.0.0");
        assert_eq!(public.quant.as_deref(), Some("Q4_K_M"));
        assert_eq!(public.context_size, Some(2048));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn earlier_root_shadows_later_one() {
        let system = temp_dir("system");
        let user = temp_dir("user");
        write_model(&user, "same.model", "1.0.0");
        write_model(&system, "same.model", "1.0.0");
        let models = scan(&[user.clone(), system.clone()]);
        assert_eq!(models.len(), 1);
        assert!(models[0].base_path.starts_with(&user));
        std::fs::remove_dir_all(user).unwrap();
        std::fs::remove_dir_all(system).unwrap();
    }

    #[test]
    fn invalid_manifest_is_skipped() {
        let root = temp_dir("invalid");
        let directory = root.join("broken");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("manifest.json"), b"{}").unwrap();
        assert!(scan(std::slice::from_ref(&root)).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_artifact_is_an_error() {
        let root = temp_dir("missing");
        let directory = root.join("model");
        std::fs::create_dir_all(&directory).unwrap();
        let manifest = ModelManifest {
            manifest_version: MODEL_MANIFEST_VERSION,
            package_id: "m".into(),
            package_version: "1".into(),
            description: String::new(),
            base: ModelArtifact {
                file: "base.gguf".into(),
                sha256: "a".repeat(64),
                size_bytes: None,
            },
            adapter: None,
            context_size: 2048,
            quant: String::new(),
            capabilities: vec![],
            operation_registry_version: None,
            training: None,
            publisher: None,
        };
        std::fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(load(&directory).unwrap_err().contains("missing"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
