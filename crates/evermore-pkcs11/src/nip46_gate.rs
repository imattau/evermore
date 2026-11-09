//! Gates `C_Decrypt` on a live NIP-46 approval round-trip with the
//! device's paired remote signer (e.g. a phone app) -- the actual
//! Phase 5.5 priority-1 unlock mechanism this whole crate exists to
//! provide. `functions.rs`'s real RSA-OAEP math only ever runs after
//! `approve_unlock` returns `Ok(())`.
//!
//! Two Nostr identities are involved here, both already established
//! elsewhere in the project rather than invented for this module:
//!
//! - the device-root identity: `evermore-signer`'s `device_identity.rs`
//!   bootstraps and owns `/var/lib/evermore/device-identity` (a
//!   root-owned, 0600, hex-encoded secret key -- same trust boundary as
//!   an SSH host key). This module runs inside `systemd-homework`, with
//!   no running `evermore-signer` daemon or session keyring to call
//!   into, so it reads that same file directly rather than depending on
//!   `evermore-signer` as a library or duplicating its bootstrap logic
//!   (bootstrapping stays that crate's job; this one only ever reads).
//! - the paired remote signer's pubkey + relay: configured separately
//!   from `evermore-signer`'s per-identity `backing::Backing::Remote`
//!   records, because this gates the *device's own* unlock rather than
//!   any one human identity's signing, via `pkcs11-nip46-pairing.json`
//!   (or `EVERMORE_PKCS11_NIP46_RELAY`/`EVERMORE_PKCS11_NIP46_REMOTE_PUBKEY`
//!   for testing).
//!
//! The approval itself rides on NIP-46's standard `sign_event` method,
//! not an invented one: the remote signer is asked to sign a small,
//! human-readable event describing the unlock request, which is exactly
//! the action real signer apps (Amber, nsec.app, ...) already show the
//! user a confirmation prompt for. A `get_public_key` round-trip comes
//! first to learn the pubkey the signer will actually sign with --
//! per NIP-46 that need not equal the paired `remote_signer_pubkey`
//! (the bunker/session key), even though in practice it usually does.
//! Receiving back a correctly-signed, correctly-addressed response
//! within the timeout is the approval; an `error` result, a malformed
//! response, or a timeout are all treated as a decline -- fail closed.

use nostr::prelude::*;
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// Custom event kind for the unlock-approval request this module asks
/// the remote signer to sign. In the "ephemeral/custom" range, chosen
/// to not collide with any NIP-defined kind.
const UNLOCK_APPROVAL_KIND: u16 = 31337;

const GET_PUBLIC_KEY_TIMEOUT: Duration = Duration::from_secs(20);
const SIGN_EVENT_TIMEOUT: Duration = Duration::from_secs(60);

struct Pairing {
    relay: String,
    remote_signer_pubkey: PublicKey,
}

#[derive(serde::Deserialize)]
struct PairingFile {
    relay: String,
    remote_signer_pubkey: String,
}

fn state_dir() -> PathBuf {
    std::env::var("EVERMORE_DEVICE_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/var/lib/evermore"))
}

fn load_pairing() -> Result<Pairing, String> {
    if let (Ok(relay), Ok(pk_hex)) = (
        std::env::var("EVERMORE_PKCS11_NIP46_RELAY"),
        std::env::var("EVERMORE_PKCS11_NIP46_REMOTE_PUBKEY"),
    ) {
        let remote_signer_pubkey = PublicKey::from_hex(&pk_hex)
            .map_err(|e| format!("invalid EVERMORE_PKCS11_NIP46_REMOTE_PUBKEY: {e}"))?;
        return Ok(Pairing {
            relay,
            remote_signer_pubkey,
        });
    }

    let path = state_dir().join("pkcs11-nip46-pairing.json");
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "no NIP-46 pairing configured (reading {}: {e}) -- unlock via this path refuses by \
             design until a remote signer is paired",
            path.display()
        )
    })?;
    let raw: PairingFile =
        serde_json::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))?;
    let remote_signer_pubkey = PublicKey::from_hex(&raw.remote_signer_pubkey)
        .map_err(|e| format!("invalid remote_signer_pubkey in {}: {e}", path.display()))?;
    Ok(Pairing {
        relay: raw.relay,
        remote_signer_pubkey,
    })
}

/// Reads the device-root identity directly from
/// `evermore-signer`'s state file. Read-only: if it doesn't exist, the
/// device hasn't been bootstrapped yet and this errors rather than
/// generating a second, competing device key.
fn load_device_keys() -> Result<Keys, String> {
    let path = state_dir().join("device-identity");
    let hex = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "reading {}: {e} -- run `evermore-signer bootstrap-device` first",
            path.display()
        )
    })?;
    let secret_key =
        SecretKey::parse(hex.trim()).map_err(|e| format!("invalid device identity key: {e}"))?;
    Ok(Keys::new(secret_key))
}

fn connect_and_subscribe(
    relay_url: &str,
    our_pubkey: PublicKey,
    remote_pubkey: PublicKey,
) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, String> {
    let (mut ws, _response) = tungstenite::connect(relay_url)
        .map_err(|e| format!("couldn't connect to relay at {relay_url}: {e}"))?;

    match ws.get_ref() {
        MaybeTlsStream::Plain(tcp) => {
            let _ = tcp.set_read_timeout(Some(Duration::from_millis(200)));
        }
        MaybeTlsStream::Rustls(tls) => {
            let _ = tls.sock.set_read_timeout(Some(Duration::from_millis(200)));
        }
        _ => {}
    }

    let filter = Filter::new()
        .kind(Kind::NostrConnect)
        .author(remote_pubkey)
        .pubkey(our_pubkey)
        .since(Timestamp::now() - 5);
    let req = serde_json::json!(["REQ", "evermore-pkcs11-nip46", filter]);
    ws.send(Message::text(req.to_string()))
        .map_err(|e| format!("subscribing on {relay_url}: {e}"))?;

    Ok(ws)
}

/// Sends `request` to `remote_pubkey` and blocks (up to `timeout`) for
/// the matching response. Any other traffic on the socket (other
/// subscriptions' events, NOTICE, EOSE, a response to some other
/// request id) is silently skipped rather than treated as an error --
/// a relay is free to interleave anything.
fn request_blocking(
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    our_keys: &Keys,
    remote_pubkey: PublicKey,
    request: NostrConnectRequest,
    timeout: Duration,
) -> Result<NostrConnectResponse, String> {
    let method = request.method();
    let message = NostrConnectMessage::request(&request);
    let req_id = message.id().to_string();

    let event = NostrConnectEventBuilder::new(remote_pubkey, message)
        .finalize(our_keys)
        .map_err(|e| format!("encrypting/signing NIP-46 request: {e}"))?;
    ws.send(Message::text(
        serde_json::json!(["EVENT", event]).to_string(),
    ))
    .map_err(|e| format!("publishing NIP-46 request: {e}"))?;

    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            return Err("timed out waiting for the remote signer".to_string());
        }
        match ws.read() {
            Ok(Message::Text(text)) => {
                if let Some(response) =
                    try_parse_response(&text, our_keys, remote_pubkey, &req_id, method)
                {
                    return Ok(response);
                }
            }
            Ok(Message::Close(_)) => return Err("relay closed the connection".to_string()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(format!("relay connection error: {e}")),
        }
    }
}

fn try_parse_response(
    text: &str,
    our_keys: &Keys,
    remote_pubkey: PublicKey,
    req_id: &str,
    method: NostrConnectMethod,
) -> Option<NostrConnectResponse> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get(0).and_then(|v| v.as_str()) != Some("EVENT") {
        return None;
    }
    let event: Event = serde_json::from_value(value.get(2)?.clone()).ok()?;
    if event.kind != Kind::NostrConnect || event.pubkey != remote_pubkey {
        return None;
    }
    let plaintext = our_keys
        .nip44_decrypt(&remote_pubkey, &event.content)
        .ok()?;
    let parsed = NostrConnectMessage::from_json(&plaintext).ok()?;
    if parsed.id() != req_id || !parsed.is_response() {
        return None;
    }
    parsed.to_response(method).ok()
}

/// Runs the full approval round-trip: `get_public_key` to learn who
/// will actually sign, then `sign_event` on a small event describing
/// this specific unlock attempt, which is exactly the action a real
/// NIP-46 signer app prompts its user to confirm. Returns `Ok(())` only
/// if a validly-signed, correctly-addressed response comes back within
/// the timeout; anything else -- decline, timeout, mismatch -- is an
/// `Err`, which `functions.rs` turns into a decrypt failure.
pub fn approve_unlock() -> Result<(), String> {
    let pairing = load_pairing()?;
    let our_keys = load_device_keys()?;
    let device_pubkey_hex = our_keys.public_key().to_hex();
    let mut ws = connect_and_subscribe(
        &pairing.relay,
        our_keys.public_key(),
        pairing.remote_signer_pubkey,
    )?;

    let pubkey_response = request_blocking(
        &mut ws,
        &our_keys,
        pairing.remote_signer_pubkey,
        NostrConnectRequest::GetPublicKey,
        GET_PUBLIC_KEY_TIMEOUT,
    )?;
    if let Some(err) = pubkey_response.error {
        return Err(format!("remote signer errored on get_public_key: {err}"));
    }
    let signer_pubkey = match pubkey_response.result {
        Some(ResponseResult::GetPublicKey(pk)) => pk,
        _ => return Err("remote signer gave no usable public key".to_string()),
    };

    let nonce = {
        use rand::RngCore;
        let mut bytes = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut bytes);
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    let content =
        format!("evermore: approve LUKS unlock for device {device_pubkey_hex} (nonce {nonce})");
    let mut unsigned = UnsignedEvent::new(
        signer_pubkey,
        Timestamp::now(),
        Kind::from(UNLOCK_APPROVAL_KIND),
        [Tag::identifier(nonce)],
        content,
    );
    unsigned.ensure_id();
    // Pin the expected id *before* sending: `EventId::compute` hashes
    // pubkey+created_at+kind+tags+content together, so checking the
    // response's id against this one below is sufficient on its own to
    // prove the remote signer signed exactly this request -- not some
    // other kind/content/tag combination that happens to still verify.
    let expected_id = unsigned.id.expect("ensure_id just set it");

    let sign_response = request_blocking(
        &mut ws,
        &our_keys,
        pairing.remote_signer_pubkey,
        NostrConnectRequest::SignEvent(unsigned),
        SIGN_EVENT_TIMEOUT,
    )?;
    if let Some(err) = sign_response.error {
        return Err(format!("remote signer declined the unlock: {err}"));
    }
    let signed = match sign_response.result {
        Some(ResponseResult::SignEvent(event)) => *event,
        _ => return Err("remote signer gave no usable signature".to_string()),
    };

    signed
        .verify()
        .map_err(|e| format!("approval event failed signature verification: {e}"))?;
    if signed.id != expected_id {
        return Err("approval event doesn't match what was requested".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    //! Exercises the real protocol round-trip end to end: a throwaway,
    //! unfiltered relay spun up just for this test, a mock remote signer
    //! that plays the "phone" side for real (its own keypair, its own
    //! connection, talking only NIP-46 wire protocol with no shortcut
    //! back into this module's internals), and `approve_unlock` itself.
    //!
    //! This deliberately does *not* reuse the project's local
    //! control-plane relay (`relay/main.go`, `ws://127.0.0.1:4869`):
    //! that relay is loopback-only by design and enforces a kind
    //! allowlist that excludes 24133 (NIP-46) on purpose, since it's
    //! for intra-device daemon traffic, not for talking to an external
    //! phone -- which is also precisely why production's `relay` field
    //! in the pairing config must always point at a real, externally
    //! reachable relay the paired phone can also see, never at that
    //! one. A throwaway relay here keeps the test hermetic and able to
    //! run anywhere, not just on the project's VM.

    use super::*;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::Sender;

    /// A minimal NIP-01 relay: no storage, no filter matching -- just
    /// broadcasts every published `EVENT` to every connected client
    /// (including the publisher, which the test's own pubkey checks
    /// already filter out) and acks `REQ` with an immediate `EOSE`.
    /// Enough to prove the real NIP-46 request/response wire protocol
    /// works; a protocol round-trip test has no need for a smarter one.
    fn spawn_test_relay() -> (String, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test relay");
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let peers: Arc<Mutex<Vec<Sender<String>>>> = Arc::new(Mutex::new(Vec::new()));

        listener.set_nonblocking(true).expect("set nonblocking");
        let stop_accept = stop.clone();
        std::thread::spawn(move || {
            while !stop_accept.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let peers = peers.clone();
                        let stop = stop_accept.clone();
                        std::thread::spawn(move || test_relay_conn(stream, peers, stop));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        });

        (format!("ws://127.0.0.1:{port}"), stop)
    }

    fn test_relay_conn(
        stream: TcpStream,
        peers: Arc<Mutex<Vec<Sender<String>>>>,
        stop: Arc<AtomicBool>,
    ) {
        let Ok(mut ws) = tungstenite::accept(stream) else {
            return;
        };
        let _ = ws
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(100)));
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        peers.lock().unwrap().push(tx);

        while !stop.load(Ordering::Relaxed) {
            match ws.read() {
                Ok(Message::Text(text)) => {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                        match value.get(0).and_then(|v| v.as_str()) {
                            Some("REQ") => {
                                if let Some(sub_id) = value.get(1).and_then(|v| v.as_str()) {
                                    let _ = ws.send(Message::text(
                                        serde_json::json!(["EOSE", sub_id]).to_string(),
                                    ));
                                }
                            }
                            Some("EVENT") => {
                                if let Some(event) = value.get(1) {
                                    let id = event.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                    let _ = ws.send(Message::text(
                                        serde_json::json!(["OK", id, true, ""]).to_string(),
                                    ));
                                    let broadcast =
                                        serde_json::json!(["EVENT", "test-relay", event])
                                            .to_string();
                                    for peer in peers.lock().unwrap().iter() {
                                        let _ = peer.send(broadcast.clone());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
            while let Ok(msg) = rx.try_recv() {
                if ws.send(Message::text(msg)).is_err() {
                    return;
                }
            }
        }
    }

    /// Plays the remote signer: answers `get_public_key` with its own
    /// key and `sign_event` by actually signing whatever unsigned event
    /// it's asked to sign, exactly like a real signer app would for a
    /// request its user approved.
    fn run_mock_signer(
        relay_url: &str,
        remote_keys: Keys,
        device_pubkey: PublicKey,
        stop: Arc<AtomicBool>,
        ready: Sender<()>,
    ) {
        let (mut ws, _) = tungstenite::connect(relay_url).expect("mock signer connect");
        if let MaybeTlsStream::Plain(tcp) = ws.get_ref() {
            let _ = tcp.set_read_timeout(Some(Duration::from_millis(200)));
        }
        let filter = Filter::new()
            .kind(Kind::NostrConnect)
            .author(device_pubkey)
            .pubkey(remote_keys.public_key())
            .since(Timestamp::now() - 5);
        let req = serde_json::json!(["REQ", "mock-signer", filter]);
        ws.send(Message::text(req.to_string()))
            .expect("mock signer subscribe");
        // Nostr Connect events are ephemeral (kind 24133 falls in NIP-16's
        // 20000-29999 range) -- relays aren't expected to persist them, so
        // unlike a normal filtered query there's no stored backlog to fall
        // back on if `approve_unlock`'s publish races ahead of this
        // subscription actually being live. Signal the caller only once
        // the REQ has actually been sent, so the test can't start the
        // real round-trip before this mock signer could possibly see it.
        let _ = ready.send(());

        while !stop.load(Ordering::Relaxed) {
            match ws.read() {
                Ok(Message::Text(text)) => {
                    let Some((req_id, request)) =
                        parse_incoming_request(&text, &remote_keys, device_pubkey)
                    else {
                        continue;
                    };
                    let result = match request {
                        NostrConnectRequest::GetPublicKey => {
                            ResponseResult::GetPublicKey(remote_keys.public_key())
                        }
                        NostrConnectRequest::SignEvent(unsigned) => {
                            let signed = unsigned
                                .finalize(&remote_keys)
                                .expect("mock signer: signing a valid unsigned event");
                            ResponseResult::SignEvent(Box::new(signed))
                        }
                        _ => continue,
                    };
                    let response_message = NostrConnectMessage::response(
                        req_id,
                        NostrConnectResponse::with_result(result),
                    );
                    let event = NostrConnectEventBuilder::new(device_pubkey, response_message)
                        .finalize(&remote_keys)
                        .expect("mock signer: building response event");
                    ws.send(Message::text(
                        serde_json::json!(["EVENT", event]).to_string(),
                    ))
                    .expect("mock signer: publishing response");
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
        }
    }

    fn parse_incoming_request(
        text: &str,
        remote_keys: &Keys,
        device_pubkey: PublicKey,
    ) -> Option<(String, NostrConnectRequest)> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        if value.get(0).and_then(|v| v.as_str()) != Some("EVENT") {
            return None;
        }
        let event: Event = serde_json::from_value(value.get(2)?.clone()).ok()?;
        if event.kind != Kind::NostrConnect || event.pubkey != device_pubkey {
            return None;
        }
        let plaintext = remote_keys
            .nip44_decrypt(&device_pubkey, &event.content)
            .ok()?;
        let message = NostrConnectMessage::from_json(&plaintext).ok()?;
        let id = message.id().to_string();
        let request = message.to_request().ok()?;
        Some((id, request))
    }

    #[test]
    fn full_round_trip_with_mock_signer() {
        let (relay_url, relay_stop) = spawn_test_relay();

        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("EVERMORE_DEVICE_STATE_DIR", tmp.path());
        }
        let device_keys = Keys::generate();
        std::fs::write(
            tmp.path().join("device-identity"),
            device_keys.secret_key().to_secret_hex(),
        )
        .unwrap();

        let remote_keys = Keys::generate();
        unsafe {
            std::env::set_var("EVERMORE_PKCS11_NIP46_RELAY", &relay_url);
            std::env::set_var(
                "EVERMORE_PKCS11_NIP46_REMOTE_PUBKEY",
                remote_keys.public_key().to_hex(),
            );
        }

        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let signer_handle = {
            let relay_url = relay_url.clone();
            let remote_keys = remote_keys.clone();
            let device_pubkey = device_keys.public_key();
            let stop = stop.clone();
            std::thread::spawn(move || {
                run_mock_signer(&relay_url, remote_keys, device_pubkey, stop, ready_tx)
            })
        };
        ready_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("mock signer never subscribed");
        // The REQ having been *sent* doesn't mean the relay has
        // *registered* it yet; give it a beat before publishing the
        // real request these ephemeral events depend on being seen live.
        std::thread::sleep(Duration::from_millis(300));

        let result = approve_unlock();
        stop.store(true, Ordering::Relaxed);
        relay_stop.store(true, Ordering::Relaxed);
        let _ = signer_handle.join();

        assert!(result.is_ok(), "approve_unlock failed: {result:?}");
    }

    #[test]
    fn fails_closed_with_no_pairing_configured() {
        unsafe {
            std::env::remove_var("EVERMORE_PKCS11_NIP46_RELAY");
            std::env::remove_var("EVERMORE_PKCS11_NIP46_REMOTE_PUBKEY");
        }
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("EVERMORE_DEVICE_STATE_DIR", tmp.path());
        }
        assert!(approve_unlock().is_err());
    }
}
