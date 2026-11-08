//! The actual `extern "C" fn C_*` implementations referenced by the
//! static function table in `lib.rs`. Everything homed's enrollment and
//! unlock flow actually exercises (verified against real homed debug
//! output on the VM, not guessed) is implemented for real; everything
//! else returns `CKR_FUNCTION_NOT_SUPPORTED`.

use crate::ffi::*;
use crate::state::{self, PRIVATE_KEY_HANDLE, PUBLIC_KEY_HANDLE};
use rsa::traits::PublicKeyParts;
use rsa::{Oaep, RsaPublicKey};
use std::os::raw::c_uchar;
use std::slice;

const SLOT_ID: CK_SLOT_ID = 1;

/// Writes `src` into the caller-provided `(out_ptr, out_len)` buffer
/// following the standard PKCS#11 "query, then fill" convention: a NULL
/// `out_ptr` just reports the required length; a non-NULL one must
/// already point at a buffer at least that large.
unsafe fn write_out(src: &[u8], out_ptr: *mut CK_BYTE, out_len: *mut CK_ULONG) -> CK_RV {
    unsafe {
        if out_ptr.is_null() {
            *out_len = src.len() as CK_ULONG;
            return CKR_OK;
        }
        if (*out_len as usize) < src.len() {
            *out_len = src.len() as CK_ULONG;
            return CKR_BUFFER_TOO_SMALL;
        }
        std::ptr::copy_nonoverlapping(src.as_ptr(), out_ptr, src.len());
        *out_len = src.len() as CK_ULONG;
        CKR_OK
    }
}

fn pad_bytes(dst: &mut [c_uchar], src: &[u8]) {
    let n = src.len().min(dst.len());
    dst[..n].copy_from_slice(&src[..n]);
}

pub extern "C" fn c_initialize(_args: CK_VOID_PTR) -> CK_RV {
    CKR_OK
}

pub extern "C" fn c_finalize(_args: CK_VOID_PTR) -> CK_RV {
    CKR_OK
}

pub extern "C" fn c_get_info(info: *mut CK_INFO) -> CK_RV {
    if info.is_null() {
        return CKR_ARGUMENTS_BAD;
    }
    unsafe {
        let info = &mut *info;
        info.cryptoki_version = CK_VERSION {
            major: 2,
            minor: 40,
        };
        info.manufacturer_id = [b' '; 32];
        pad_bytes(&mut info.manufacturer_id, b"Evermore");
        info.flags = 0;
        info.library_description = [b' '; 32];
        pad_bytes(
            &mut info.library_description,
            b"Evermore NIP-46 PKCS11 shim",
        );
        info.library_version = CK_VERSION { major: 0, minor: 1 };
    }
    CKR_OK
}

pub extern "C" fn c_get_slot_list(
    _token_present: CK_BBOOL,
    slot_list: *mut CK_SLOT_ID,
    count: *mut CK_ULONG,
) -> CK_RV {
    if count.is_null() {
        return CKR_ARGUMENTS_BAD;
    }
    unsafe {
        if slot_list.is_null() {
            *count = 1;
            return CKR_OK;
        }
        if *count < 1 {
            *count = 1;
            return CKR_BUFFER_TOO_SMALL;
        }
        *slot_list = SLOT_ID;
        *count = 1;
    }
    CKR_OK
}

pub extern "C" fn c_get_slot_info(slot_id: CK_SLOT_ID, info: *mut CK_SLOT_INFO) -> CK_RV {
    if slot_id != SLOT_ID || info.is_null() {
        return CKR_ARGUMENTS_BAD;
    }
    unsafe {
        let info = &mut *info;
        info.slot_description = [b' '; 64];
        pad_bytes(&mut info.slot_description, b"Evermore virtual NIP-46 slot");
        info.manufacturer_id = [b' '; 32];
        pad_bytes(&mut info.manufacturer_id, b"Evermore");
        info.flags = CKF_TOKEN_PRESENT;
        info.hardware_version = CK_VERSION { major: 0, minor: 1 };
        info.firmware_version = CK_VERSION { major: 0, minor: 1 };
    }
    CKR_OK
}

pub extern "C" fn c_get_token_info(slot_id: CK_SLOT_ID, info: *mut CK_TOKEN_INFO) -> CK_RV {
    if slot_id != SLOT_ID || info.is_null() {
        return CKR_ARGUMENTS_BAD;
    }
    unsafe {
        let info = &mut *info;
        info.label = [b' '; 32];
        pad_bytes(&mut info.label, b"evermore-nip46");
        info.manufacturer_id = [b' '; 32];
        pad_bytes(&mut info.manufacturer_id, b"Evermore");
        info.model = [b' '; 16];
        pad_bytes(&mut info.model, b"nip46-shim");
        info.serial_number = [b' '; 16];
        pad_bytes(&mut info.serial_number, b"0001");
        info.flags = CKF_TOKEN_PRESENT | CKF_RNG | CKF_LOGIN_REQUIRED | CKF_USER_PIN_INITIALIZED;
        info.max_session_count = 8;
        info.session_count = 0;
        info.max_rw_session_count = 0;
        info.rw_session_count = 0;
        info.max_pin_len = 64;
        info.min_pin_len = 1;
        info.total_public_memory = 0;
        info.free_public_memory = 0;
        info.total_private_memory = 0;
        info.free_private_memory = 0;
        info.hardware_version = CK_VERSION { major: 0, minor: 1 };
        info.firmware_version = CK_VERSION { major: 0, minor: 1 };
        info.utc_time = [b' '; 16];
    }
    CKR_OK
}

pub extern "C" fn c_get_mechanism_list(
    slot_id: CK_SLOT_ID,
    list: *mut CK_MECHANISM_TYPE,
    count: *mut CK_ULONG,
) -> CK_RV {
    if slot_id != SLOT_ID || count.is_null() {
        return CKR_ARGUMENTS_BAD;
    }
    unsafe {
        if list.is_null() {
            *count = 1;
            return CKR_OK;
        }
        if *count < 1 {
            *count = 1;
            return CKR_BUFFER_TOO_SMALL;
        }
        *list = CKM_RSA_PKCS_OAEP;
        *count = 1;
    }
    CKR_OK
}

pub extern "C" fn c_get_mechanism_info(
    slot_id: CK_SLOT_ID,
    mechanism: CK_MECHANISM_TYPE,
    info: *mut CK_MECHANISM_INFO,
) -> CK_RV {
    const CKF_DECRYPT: CK_FLAGS = 1 << 9;
    if slot_id != SLOT_ID || mechanism != CKM_RSA_PKCS_OAEP || info.is_null() {
        return CKR_MECHANISM_INVALID;
    }
    unsafe {
        let info = &mut *info;
        info.min_key_size = 2048;
        info.max_key_size = 2048;
        info.flags = CKF_DECRYPT;
    }
    CKR_OK
}

pub extern "C" fn c_open_session(
    slot_id: CK_SLOT_ID,
    _flags: CK_FLAGS,
    _application: CK_VOID_PTR,
    _notify: CK_VOID_PTR,
    session: *mut CK_SESSION_HANDLE,
) -> CK_RV {
    if slot_id != SLOT_ID || session.is_null() {
        return CKR_ARGUMENTS_BAD;
    }
    let mut provider = state::provider().lock().unwrap();
    let handle = provider.next_session_handle;
    provider.next_session_handle += 1;
    provider.sessions.insert(handle, state::Session::default());
    unsafe {
        *session = handle;
    }
    CKR_OK
}

pub extern "C" fn c_close_session(session: CK_SESSION_HANDLE) -> CK_RV {
    let mut provider = state::provider().lock().unwrap();
    provider.sessions.remove(&session);
    CKR_OK
}

pub extern "C" fn c_close_all_sessions(_slot_id: CK_SLOT_ID) -> CK_RV {
    let mut provider = state::provider().lock().unwrap();
    provider.sessions.clear();
    CKR_OK
}

pub extern "C" fn c_get_session_info(
    session: CK_SESSION_HANDLE,
    info: *mut CK_SESSION_INFO,
) -> CK_RV {
    if info.is_null() {
        return CKR_ARGUMENTS_BAD;
    }
    let provider = state::provider().lock().unwrap();
    let Some(s) = provider.sessions.get(&session) else {
        return CKR_SESSION_HANDLE_INVALID;
    };
    unsafe {
        let info = &mut *info;
        info.slot_id = SLOT_ID;
        info.state = if s.logged_in {
            CKS_RO_USER_FUNCTIONS
        } else {
            CKS_RO_PUBLIC_SESSION
        };
        info.flags = CKF_SERIAL_SESSION;
        info.device_error = 0;
    }
    CKR_OK
}

pub extern "C" fn c_login(
    session: CK_SESSION_HANDLE,
    _user_type: CK_USER_TYPE,
    pin: *mut CK_BYTE,
    pin_len: CK_ULONG,
) -> CK_RV {
    // The PIN is deliberately not the real security gate here -- it
    // never will be. NIP-46 approval is. Any non-empty PIN is accepted
    // so homed's existing PIN-entry UX keeps working unmodified; the
    // actual gate belongs in `c_decrypt` once the NIP-46 round-trip
    // exists (see that function's doc comment).
    if pin.is_null() || pin_len == 0 {
        return CKR_PIN_INCORRECT;
    }
    let mut provider = state::provider().lock().unwrap();
    let Some(s) = provider.sessions.get_mut(&session) else {
        return CKR_SESSION_HANDLE_INVALID;
    };
    s.logged_in = true;
    CKR_OK
}

pub extern "C" fn c_logout(session: CK_SESSION_HANDLE) -> CK_RV {
    let mut provider = state::provider().lock().unwrap();
    let Some(s) = provider.sessions.get_mut(&session) else {
        return CKR_SESSION_HANDLE_INVALID;
    };
    s.logged_in = false;
    CKR_OK
}

/// Reads a `CKA_CLASS` value out of a find-objects template, if present.
unsafe fn template_class(template: *mut CK_ATTRIBUTE, count: CK_ULONG) -> Option<CK_OBJECT_CLASS> {
    unsafe {
        let attrs = slice::from_raw_parts(template, count as usize);
        for attr in attrs {
            if attr.attribute_type == CKA_CLASS && !attr.value.is_null() {
                let ptr = attr.value as *const CK_OBJECT_CLASS;
                return Some(*ptr);
            }
        }
        None
    }
}

pub extern "C" fn c_find_objects_init(
    session: CK_SESSION_HANDLE,
    template: *mut CK_ATTRIBUTE,
    count: CK_ULONG,
) -> CK_RV {
    let class = if template.is_null() || count == 0 {
        None
    } else {
        unsafe { template_class(template, count) }
    };
    let results = match class {
        Some(c) if c == CKO_PUBLIC_KEY => vec![PUBLIC_KEY_HANDLE],
        Some(c) if c == CKO_PRIVATE_KEY => vec![PRIVATE_KEY_HANDLE],
        _ => vec![PUBLIC_KEY_HANDLE, PRIVATE_KEY_HANDLE],
    };
    let mut provider = state::provider().lock().unwrap();
    let Some(s) = provider.sessions.get_mut(&session) else {
        return CKR_SESSION_HANDLE_INVALID;
    };
    s.find_results = results;
    CKR_OK
}

pub extern "C" fn c_find_objects(
    session: CK_SESSION_HANDLE,
    object_list: *mut CK_OBJECT_HANDLE,
    max_count: CK_ULONG,
    count: *mut CK_ULONG,
) -> CK_RV {
    if count.is_null() {
        return CKR_ARGUMENTS_BAD;
    }
    let mut provider = state::provider().lock().unwrap();
    let Some(s) = provider.sessions.get_mut(&session) else {
        return CKR_SESSION_HANDLE_INVALID;
    };
    let n = (max_count as usize).min(s.find_results.len());
    let returned: Vec<u64> = s.find_results.drain(..n).collect();
    unsafe {
        for (i, handle) in returned.into_iter().enumerate() {
            *object_list.add(i) = handle;
        }
        *count = n as CK_ULONG;
    }
    CKR_OK
}

pub extern "C" fn c_find_objects_final(_session: CK_SESSION_HANDLE) -> CK_RV {
    CKR_OK
}

pub extern "C" fn c_get_attribute_value(
    _session: CK_SESSION_HANDLE,
    object: CK_OBJECT_HANDLE,
    template: *mut CK_ATTRIBUTE,
    count: CK_ULONG,
) -> CK_RV {
    if object != PUBLIC_KEY_HANDLE && object != PRIVATE_KEY_HANDLE {
        return CKR_OBJECT_HANDLE_INVALID;
    }
    let provider = state::provider().lock().unwrap();
    let public_key = RsaPublicKey::from(&provider.key);
    let modulus = public_key.n().to_bytes_be();
    let exponent = public_key.e().to_bytes_be();
    drop(provider);

    let is_private = object == PRIVATE_KEY_HANDLE;
    let mut overall = CKR_OK;

    unsafe {
        let attrs = slice::from_raw_parts_mut(template, count as usize);
        for attr in attrs {
            let rv = match attr.attribute_type {
                CKA_CLASS => {
                    let class: CK_OBJECT_CLASS = if is_private {
                        CKO_PRIVATE_KEY
                    } else {
                        CKO_PUBLIC_KEY
                    };
                    write_out(
                        &class.to_ne_bytes(),
                        attr.value as *mut CK_BYTE,
                        &mut attr.value_len,
                    )
                }
                CKA_KEY_TYPE => write_out(
                    &CKK_RSA.to_ne_bytes(),
                    attr.value as *mut CK_BYTE,
                    &mut attr.value_len,
                ),
                CKA_LABEL => write_out(b"evtest", attr.value as *mut CK_BYTE, &mut attr.value_len),
                CKA_ID => write_out(&[1u8], attr.value as *mut CK_BYTE, &mut attr.value_len),
                CKA_MODULUS => write_out(&modulus, attr.value as *mut CK_BYTE, &mut attr.value_len),
                CKA_PUBLIC_EXPONENT => {
                    write_out(&exponent, attr.value as *mut CK_BYTE, &mut attr.value_len)
                }
                CKA_DECRYPT => {
                    let v: CK_BBOOL = if is_private { 1 } else { 0 };
                    write_out(&[v], attr.value as *mut CK_BYTE, &mut attr.value_len)
                }
                CKA_TOKEN => write_out(&[1u8], attr.value as *mut CK_BYTE, &mut attr.value_len),
                CKA_PRIVATE => {
                    let v: CK_BBOOL = if is_private { 1 } else { 0 };
                    write_out(&[v], attr.value as *mut CK_BYTE, &mut attr.value_len)
                }
                _ => {
                    attr.value_len = u64::MAX as CK_ULONG;
                    CKR_ATTRIBUTE_TYPE_INVALID
                }
            };
            if rv != CKR_OK {
                overall = rv;
            }
        }
    }
    overall
}

pub extern "C" fn c_decrypt_init(
    session: CK_SESSION_HANDLE,
    mechanism: *mut CK_MECHANISM,
    key: CK_OBJECT_HANDLE,
) -> CK_RV {
    if mechanism.is_null() || key != PRIVATE_KEY_HANDLE {
        return CKR_ARGUMENTS_BAD;
    }
    let mechanism_type = unsafe { (*mechanism).mechanism };
    if mechanism_type != CKM_RSA_PKCS_OAEP {
        return CKR_MECHANISM_INVALID;
    }

    // `homed` probes which OAEP hash the token supports and picks one
    // per-token (verified live against real homed: it chose SHA-256
    // against this module's own mechanism list) -- the mechanism
    // parameter carries that choice, and OAEP decryption fails outright
    // if the wrong hash is used, so this has to be read, not assumed.
    let parameter = unsafe { (*mechanism).parameter };
    let parameter_len = unsafe { (*mechanism).parameter_len };
    if parameter.is_null()
        || parameter_len as usize != std::mem::size_of::<CK_RSA_PKCS_OAEP_PARAMS>()
    {
        return CKR_ARGUMENTS_BAD;
    }
    let oaep_params = unsafe { &*(parameter as *const CK_RSA_PKCS_OAEP_PARAMS) };
    let hash = match oaep_params.hash_alg {
        CKM_SHA_1 => state::OaepHash::Sha1,
        CKM_SHA256 => state::OaepHash::Sha256,
        _ => return CKR_MECHANISM_INVALID,
    };

    let mut provider = state::provider().lock().unwrap();
    let Some(s) = provider.sessions.get_mut(&session) else {
        return CKR_SESSION_HANDLE_INVALID;
    };
    if !s.logged_in {
        return CKR_USER_NOT_LOGGED_IN;
    }
    s.decrypting = true;
    s.decrypt_hash = Some(hash);
    CKR_OK
}

/// The one function that actually matters for Phase 5.5's NIP-46
/// priority-1 unlock path. **Does real RSA-OAEP decryption right now
/// (SHA-1 or SHA-256, matching whatever `C_DecryptInit` negotiated) --
/// verified end-to-end against homed's own enrollment/unlock flow,
/// including a real LUKS-encrypted home actually mounting -- but does
/// not yet gate that decryption on a NIP-46 approval round-trip.** That
/// gate is the entire remaining point of this crate and belongs right
/// here, before the `rsa` decrypt call below: block on a signed
/// request/response with the paired remote signer over the relay named
/// in the identity's `backing::Backing::Remote` record, and return
/// `CKR_FUNCTION_FAILED` on a decline or timeout instead of proceeding.
/// Not implemented in this increment; this comment is the marker, not a
/// placeholder pretending it's done.
pub extern "C" fn c_decrypt(
    session: CK_SESSION_HANDLE,
    ciphertext: *mut CK_BYTE,
    ciphertext_len: CK_ULONG,
    plaintext: *mut CK_BYTE,
    plaintext_len: *mut CK_ULONG,
) -> CK_RV {
    if ciphertext.is_null() || plaintext_len.is_null() {
        return CKR_ARGUMENTS_BAD;
    }
    let provider = state::provider().lock().unwrap();
    let hash = match provider.sessions.get(&session) {
        Some(s) if s.logged_in && s.decrypting => s.decrypt_hash,
        Some(_) => return CKR_USER_NOT_LOGGED_IN,
        None => return CKR_SESSION_HANDLE_INVALID,
    };
    let Some(hash) = hash else {
        return CKR_ARGUMENTS_BAD;
    };

    let ciphertext_slice = unsafe { slice::from_raw_parts(ciphertext, ciphertext_len as usize) };
    let decrypted = match hash {
        state::OaepHash::Sha1 => provider
            .key
            .decrypt(Oaep::new::<sha1::Sha1>(), ciphertext_slice),
        state::OaepHash::Sha256 => provider
            .key
            .decrypt(Oaep::new::<sha2::Sha256>(), ciphertext_slice),
    };
    drop(provider);

    let mut provider = state::provider().lock().unwrap();
    if let Some(s) = provider.sessions.get_mut(&session) {
        s.decrypting = false;
    }
    drop(provider);

    let decrypted = match decrypted {
        Ok(bytes) => bytes,
        Err(_) => return CKR_DATA_LEN_RANGE,
    };
    unsafe { write_out(&decrypted, plaintext, plaintext_len) }
}

pub extern "C" fn c_generate_random(
    _session: CK_SESSION_HANDLE,
    out: *mut CK_BYTE,
    len: CK_ULONG,
) -> CK_RV {
    if out.is_null() {
        return CKR_ARGUMENTS_BAD;
    }
    use rand::RngCore;
    let mut buf = vec![0u8; len as usize];
    rand::thread_rng().fill_bytes(&mut buf);
    unsafe {
        std::ptr::copy_nonoverlapping(buf.as_ptr(), out, buf.len());
    }
    CKR_OK
}

pub extern "C" fn c_seed_random(
    _session: CK_SESSION_HANDLE,
    _seed: *mut CK_BYTE,
    _len: CK_ULONG,
) -> CK_RV {
    CKR_OK
}
