//! PAM authentication bridge for explicit NIP-46 phone sign-in.
//!
//! This module is loaded only by greetd. It asks the greeter for the
//! passphrase or for the dedicated phone sentinel. The latter activates the
//! homed account using its enrolled PKCS#11 token PIN; the PKCS#11 provider
//! blocks that activation until the paired signer approves its NIP-46
//! request.

use libc::{c_char, c_int, c_void};
use std::ffi::{CStr, CString};
use std::ptr;
use std::time::Duration;
use zeroize::Zeroizing;

const PAM_SUCCESS: c_int = 0;
const PAM_AUTH_ERR: c_int = 7;
const PAM_CONV_ERR: c_int = 19;
const PAM_IGNORE: c_int = 25;
const PAM_USER: c_int = 2;
const PAM_AUTHTOK: c_int = 6;
const PAM_PROMPT_ECHO_OFF: c_int = 1;

const PHONE_SENTINEL: &str = "__EVERMORE_NIP46_PHONE_LOGIN__";
const LOGIN_PROMPT: &str = "Passphrase, or choose Use phone: ";
const HOME_DESTINATION: &str = "org.freedesktop.home1";
const HOME_PATH: &str = "/org/freedesktop/home1";
const HOME_INTERFACE: &str = "org.freedesktop.home1.Manager";

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

type PamConversation = unsafe extern "C" fn(
    c_int,
    *mut *const PamMessage,
    *mut *mut PamResponse,
    *mut c_void,
) -> c_int;

#[repr(C)]
struct PamConv {
    conv: Option<PamConversation>,
    appdata_ptr: *mut c_void,
}

#[repr(C)]
pub struct PamHandle {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn pam_get_item(pamh: *const PamHandle, item: c_int, itemp: *mut *const c_void) -> c_int;
    fn pam_set_item(pamh: *const PamHandle, item: c_int, itemp: *const c_void) -> c_int;
}

fn pam_item(pamh: *const PamHandle, item: c_int) -> Result<*const c_void, c_int> {
    let mut value = ptr::null();
    let result = unsafe { pam_get_item(pamh, item, &mut value) };
    if result != PAM_SUCCESS || value.is_null() {
        return Err(result);
    }
    Ok(value)
}

fn prompt(pamh: *const PamHandle) -> Result<*mut c_char, c_int> {
    let conv_item = pam_item(pamh, 5)?;
    let conv: &PamConv = unsafe { &*conv_item.cast::<PamConv>() };
    let Some(call) = conv.conv else {
        return Err(PAM_CONV_ERR);
    };
    let message = CString::new(LOGIN_PROMPT).expect("static prompt has no NUL");
    let item = PamMessage {
        msg_style: PAM_PROMPT_ECHO_OFF,
        msg: message.as_ptr(),
    };
    let mut item_ptr = &item as *const PamMessage;
    let mut response_ptr: *mut PamResponse = ptr::null_mut();
    let result = unsafe { call(1, &mut item_ptr, &mut response_ptr, conv.appdata_ptr) };
    if result != PAM_SUCCESS || response_ptr.is_null() {
        return Err(PAM_CONV_ERR);
    }
    let response = unsafe { (*response_ptr).resp };
    unsafe { libc::free(response_ptr.cast()) };
    if response.is_null() {
        return Err(PAM_CONV_ERR);
    }
    Ok(response)
}

fn activate_for_phone(username: &str) -> Result<(), String> {
    if username.is_empty()
        || username.len() > 31
        || !username
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err("invalid homed username".into());
    }
    let username = CString::new(username).map_err(|_| "invalid homed username")?;
    let username = username
        .to_str()
        .map_err(|_| "homed username is not valid UTF-8")?;
    let connection = zbus::blocking::connection::Builder::system()
        .map_err(|e| format!("connecting to system bus: {e}"))?
        .method_timeout(Duration::from_secs(100))
        .build()
        .map_err(|e| format!("connecting to system bus: {e}"))?;
    let reply = connection
        .call_method(
            Some(HOME_DESTINATION),
            HOME_PATH,
            Some(HOME_INTERFACE),
            "GetUserRecordByName",
            &(username,),
        )
        .map_err(|e| format!("checking phone enrollment: {e}"))?;
    let (record_json, _incomplete, _path): (String, bool, zbus::zvariant::OwnedObjectPath) = reply
        .body()
        .deserialize()
        .map_err(|e| format!("reading phone enrollment: {e}"))?;
    let record: serde_json::Value =
        serde_json::from_str(&record_json).map_err(|e| format!("parsing phone enrollment: {e}"))?;
    if !record
        .get("pkcs11TokenUri")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|uris| !uris.is_empty())
    {
        return Err("this account has no phone unlock factor enrolled".into());
    }

    // systemd-homed passes this PIN to PKCS#11 C_Login. The provider uses
    // the username tag to select that user's root-managed phone pairing.
    let token_pin = format!("evermore-phone-approval:{username}");
    let secret = serde_json::json!({ "tokenPin": [token_pin] }).to_string();
    connection
        .call_method(
            Some(HOME_DESTINATION),
            HOME_PATH,
            Some(HOME_INTERFACE),
            "ActivateHome",
            &(username, secret.as_str()),
        )
        .map_err(|e| format!("phone unlock was not approved: {e}"))?;
    Ok(())
}

/// PAM authentication entry point. A regular passphrase is cached for the
/// systemd-homed/PAM stack below us. The phone sentinel instead performs a
/// complete homed activation, which invokes the configured PKCS#11 provider
/// and waits for its paired NIP-46 signer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: *mut PamHandle,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    if pamh.is_null() {
        return PAM_AUTH_ERR;
    }
    let username = match pam_item(pamh, PAM_USER) {
        Ok(value) => unsafe { CStr::from_ptr(value.cast()) }
            .to_string_lossy()
            .into_owned(),
        Err(code) => return code,
    };
    let response = match prompt(pamh) {
        Ok(response) => response,
        Err(code) => return code,
    };
    let response_text = unsafe { CStr::from_ptr(response) };
    let response_len = response_text.to_bytes().len();
    let value = Zeroizing::new(response_text.to_string_lossy().into_owned());
    unsafe { libc::explicit_bzero(response.cast(), response_len) };
    unsafe { libc::free(response.cast()) };

    if value.as_str() == PHONE_SENTINEL {
        return match activate_for_phone(&username) {
            Ok(()) => PAM_SUCCESS,
            Err(error) => {
                eprintln!("pam_evermore_phone: {error}");
                PAM_AUTH_ERR
            }
        };
    }

    if value.is_empty() {
        return PAM_AUTH_ERR;
    }
    let passphrase = match CString::new(value.as_str()) {
        Ok(value) => value,
        Err(_) => return PAM_AUTH_ERR,
    };
    let result = unsafe { pam_set_item(pamh, PAM_AUTHTOK, passphrase.as_ptr().cast()) };
    unsafe {
        libc::explicit_bzero(
            passphrase.as_ptr() as *mut c_void,
            passphrase.as_bytes().len(),
        )
    };
    if result == PAM_SUCCESS {
        PAM_IGNORE
    } else {
        result
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_setcred(
    _pamh: *mut PamHandle,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    PAM_SUCCESS
}
