//! Blossom (BUD-01/02/04/12) HTTP client: upload, download, list, mirror. Auth
//! is NIP-98 (BUD-01) -- a short-lived `kind:27235` event signed by
//! evermore-signer, base64-encoded into the `Authorization: Nostr <...>`
//! header. No API key is ever stored; every request gets a freshly signed
//! event.

use crate::signer_client::SignerClient;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// `list` isn't needed by the current sync and publish flows, but remains a
// complete API operation because M4.1 explicitly includes it.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BlobDescriptor {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub r#type: String,
    pub uploaded: u64,
}

pub struct BlossomClient<'a> {
    base_url: String,
    signer: &'a SignerClient,
}

impl<'a> BlossomClient<'a> {
    pub fn new(base_url: impl Into<String>, signer: &'a SignerClient) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            signer,
        }
    }

    pub fn upload(&self, data: &[u8], content_type: &str) -> Result<BlobDescriptor, String> {
        let url = format!("{}/upload", self.base_url);
        let sha256 = hex_sha256(data);
        let auth = self.build_auth("PUT", &url, Some(&sha256))?;

        let response = ureq::put(&url)
            .header("Authorization", &auth)
            .header("Content-Type", content_type)
            .send(data)
            .map_err(|e| format!("upload failed: {e}"))?;

        parse_json(response)
    }

    pub fn download(&self, sha256: &str) -> Result<Vec<u8>, String> {
        let url = format!("{}/{}", self.base_url, sha256);
        let mut response = ureq::get(&url)
            .call()
            .map_err(|e| format!("download failed: {e}"))?;
        response
            .body_mut()
            .read_to_vec()
            .map_err(|e| format!("reading download body: {e}"))
    }

    /// Ask this server to fetch a blob from another public URL (BUD-04).
    pub fn mirror(&self, source_url: &str, sha256: &str) -> Result<BlobDescriptor, String> {
        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("mirror requires the blob's 64-character SHA-256 hex digest".into());
        }
        let url = format!("{}/mirror", self.base_url);
        let body = serde_json::json!({"url": source_url}).to_string();
        let auth = self.build_bud11_auth("upload", sha256)?;
        let response = ureq::put(&url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(|e| format!("mirror failed: {e}"))?;
        parse_json(response)
    }

    #[allow(dead_code)] // M4.4 (file-manager "browse" integration), not built yet
    pub fn list(&self, pubkey_hex: &str) -> Result<Vec<BlobDescriptor>, String> {
        let url = format!("{}/list/{}", self.base_url, pubkey_hex);
        let auth = self.build_auth("GET", &url, None)?;
        let response = ureq::get(&url)
            .header("Authorization", &auth)
            .call()
            .map_err(|e| format!("list failed: {e}"))?;
        parse_json(response)
    }

    /// Builds and signs a NIP-98 (BUD-01) auth event for `method`/`url`,
    /// returning the `Authorization` header value (`"Nostr <base64>"`).
    fn build_auth(
        &self,
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
        let event = self.signer.sign_event(27235, String::new(), tags)?;
        let event_json = serde_json::to_string(&event).map_err(|e| e.to_string())?;
        Ok(format!(
            "Nostr {}",
            base64::engine::general_purpose::STANDARD.encode(event_json.as_bytes())
        ))
    }

    /// BUD-11 authorization for Blossom endpoints. BUD-04 mirror requests
    /// use an upload token scoped to the expected blob hash.
    fn build_bud11_auth(&self, action: &str, sha256: &str) -> Result<String, String> {
        let expiration = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + 60;
        let mut tags = vec![
            vec!["t".to_string(), action.to_string()],
            vec!["expiration".to_string(), expiration.to_string()],
            vec!["x".to_string(), sha256.to_ascii_lowercase()],
        ];
        let uri: http::Uri = self
            .base_url
            .parse()
            .map_err(|e| format!("invalid Blossom server URL: {e}"))?;
        if let Some(host) = uri
            .host()
            .filter(|host| host.parse::<std::net::IpAddr>().is_err())
        {
            tags.push(vec!["server".to_string(), host.to_ascii_lowercase()]);
        }
        let event = self
            .signer
            .sign_event(24242, "Mirror blob".to_string(), tags)?;
        let event_json = serde_json::to_string(&event).map_err(|e| e.to_string())?;
        Ok(format!(
            "Nostr {}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(event_json.as_bytes())
        ))
    }
}

pub fn hex_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn parse_json<T: for<'de> Deserialize<'de>>(
    mut response: http::Response<ureq::Body>,
) -> Result<T, String> {
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("reading response body: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("parsing response JSON ({text}): {e}"))
}
