//! `.npk` archive format: a gzipped tar with `manifest.json` at the root
//! plus whatever payload files the manifest references.

use crate::Manifest;
use std::io::Read;
use std::path::Path;

/// Builds a `.npk` (gzipped tar) from `manifest` plus every file under
/// `payload_dir`, returning the archive bytes.
pub fn build(manifest: &Manifest, payload_dir: &Path) -> Result<Vec<u8>, String> {
    let manifest_json =
        serde_json::to_vec_pretty(manifest).map_err(|e| format!("serializing manifest: {e}"))?;

    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);

    let mut header = tar::Header::new_gnu();
    header.set_size(manifest_json.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "manifest.json", manifest_json.as_slice())
        .map_err(|e| format!("writing manifest.json into archive: {e}"))?;

    if payload_dir.is_dir() {
        for entry in walk(payload_dir)? {
            let relative = entry.strip_prefix(payload_dir).map_err(|e| e.to_string())?;
            builder
                .append_path_with_name(&entry, relative)
                .map_err(|e| format!("adding {} to archive: {e}", entry.display()))?;
        }
    }

    let encoder = builder
        .into_inner()
        .map_err(|e| format!("finishing archive: {e}"))?;
    encoder
        .finish()
        .map_err(|e| format!("finishing gzip stream: {e}"))
}

/// Extracts a `.npk`'s manifest and payload files into `dest_dir`
/// (created if missing). Returns the parsed manifest.
pub fn extract(data: &[u8], dest_dir: &Path) -> Result<Manifest, String> {
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| format!("creating {}: {e}", dest_dir.display()))?;

    let decoder = flate2::read::GzDecoder::new(data);
    let mut archive = tar::Archive::new(decoder);
    let mut manifest: Option<Manifest> = None;

    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();

        if path == Path::new("manifest.json") {
            let mut contents = String::new();
            entry
                .read_to_string(&mut contents)
                .map_err(|e| e.to_string())?;
            manifest = Some(
                serde_json::from_str(&contents)
                    .map_err(|e| format!("invalid manifest.json: {e}"))?,
            );
            // Also written to disk (not just parsed into memory) -- later
            // CLI invocations (installed/remove) are separate processes
            // with no access to this call's return value, so they need to
            // read it back from the install directory.
            std::fs::write(dest_dir.join("manifest.json"), &contents).map_err(|e| e.to_string())?;
            continue;
        }

        let dest_path = safe_join(dest_dir, &path)?;
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mode = entry.header().mode().unwrap_or(0o644);
        let mut out = std::fs::File::create(&dest_path).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = out.set_permissions(std::fs::Permissions::from_mode(mode));
        }
    }

    manifest.ok_or_else(|| "archive is missing manifest.json".to_string())
}

/// Rejects any archive entry that would escape `dest_dir` via `..` or an
/// absolute path -- a `.npk` is untrusted input until its hash has been
/// checked against a signed release event, and even then there's no
/// reason to let it write outside its own install directory.
fn safe_join(dest_dir: &Path, entry_path: &Path) -> Result<std::path::PathBuf, String> {
    if entry_path.is_absolute() || entry_path.components().any(|c| c.as_os_str() == "..") {
        return Err(format!(
            "archive entry escapes its directory: {}",
            entry_path.display()
        ));
    }
    Ok(dest_dir.join(entry_path))
}

fn walk(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}
