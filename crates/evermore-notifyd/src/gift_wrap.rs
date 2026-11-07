//! NIP-17/NIP-59 gift wrap unwrapping. A gift-wrapped DM is two layers of
//! NIP-44 encryption: the outer `kind:1059` event (signed by a throwaway
//! ephemeral key, so it reveals nothing about the real sender) wraps a
//! "seal" (`kind:13`, signed by the real sender) whose own content is
//! itself NIP-44 encrypted, wrapping the actual unsigned "rumor" event.
//! Both decrypt steps go through evermore-signer -- the plaintext secret
//! key is never visible here, only decrypted payloads.

use crate::signer_client::SignerClient;
use nostr::prelude::*;

pub struct UnwrappedRumor {
    pub kind: u16,
    /// The real sender's pubkey (hex) -- recovered from the seal, not the
    /// outer gift wrap (which is signed by a throwaway ephemeral key).
    pub sender_pubkey: String,
    pub content: String,
}

pub fn unwrap(signer: &SignerClient, gift_wrap: &Event) -> Result<UnwrappedRumor, String> {
    let ephemeral_pubkey = gift_wrap.pubkey.to_hex();
    let seal_json = signer.nip44_decrypt(&ephemeral_pubkey, &gift_wrap.content)?;
    let seal: serde_json::Value =
        serde_json::from_str(&seal_json).map_err(|e| format!("seal wasn't valid JSON: {e}"))?;

    let seal_pubkey = seal
        .get("pubkey")
        .and_then(|v| v.as_str())
        .ok_or("seal missing pubkey")?
        .to_string();
    let seal_content = seal
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("seal missing content")?;

    let rumor_json = signer.nip44_decrypt(&seal_pubkey, seal_content)?;
    let rumor: serde_json::Value =
        serde_json::from_str(&rumor_json).map_err(|e| format!("rumor wasn't valid JSON: {e}"))?;

    Ok(UnwrappedRumor {
        kind: rumor.get("kind").and_then(|v| v.as_u64()).unwrap_or(0) as u16,
        sender_pubkey: seal_pubkey,
        content: rumor
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}
