//! Minimal Blossom client: upload + download only (npack doesn't need
//! list/mirror -- it finds packages via relay events, not a Blossom
//! server's own listing). NIP-98 auth signed by evermore-signer.

use crate::signer_client::SignerClient;
use base64::Engine;
use sha2::{Digest, Sha256};

pub fn hex_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

pub fn upload(base_url: &str, signer: &SignerClient, data: &[u8]) -> Result<String, String> {
    let base_url = base_url.trim_end_matches('/');
    let url = format!("{base_url}/upload");
    let sha256 = hex_sha256(data);
    let auth = build_auth(signer, "PUT", &url, Some(&sha256))?;

    let mut response = ureq::put(&url)
        .header("Authorization", &auth)
        .header("Content-Type", "application/gzip")
        .send(data)
        .map_err(|e| format!("upload failed: {e}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("reading upload response: {e}"))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("parsing upload response ({text}): {e}"))?;
    value
        .get("url")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("upload response missing url field: {text}"))
}

pub fn download(base_url: &str, sha256: &str) -> Result<Vec<u8>, String> {
    let base_url = base_url.trim_end_matches('/');
    let url = format!("{base_url}/{sha256}");
    let mut response = ureq::get(&url)
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    response
        .body_mut()
        .read_to_vec()
        .map_err(|e| format!("reading download body: {e}"))
}

fn build_auth(
    signer: &SignerClient,
    method: &str,
    url: &str,
    payload_sha256: Option<&str>,
) -> Result<String, String> {
    let mut tags = vec![
        vec!["u".to_string(), url.to_string()],
        vec!["method".to_string(), method.to_string()],
    ];
    if let Some(sha) = payload_sha256 {
        tags.push(vec!["payload".to_string(), sha.to_string()]);
    }
    let event = signer.sign_event(27235, String::new(), tags)?;
    let event_json = serde_json::to_string(&event).map_err(|e| e.to_string())?;
    Ok(format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(event_json.as_bytes())
    ))
}
