//! Named stubs for every PKCS#11 v2.40 function this provider doesn't
//! need (object creation/copying, digest/sign/verify, key
//! generation/wrapping/derivation, and the other session-state
//! functions). Each always returns `CKR_FUNCTION_NOT_SUPPORTED`.
//!
//! These have to be real `extern "C" fn` items, not closures assigned
//! inline in the function table: a Rust closure is always Rust-ABI, even
//! non-capturing ones, and does not coerce to an `extern "C" fn` pointer
//! type the way a plain `fn` item does.

use crate::ffi::*;

pub extern "C" fn stub4_slot_bytes(
    _: CK_SLOT_ID,
    _: *mut CK_BYTE,
    _: CK_ULONG,
    _: *mut CK_BYTE,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub3_session_bytes_len(
    _: CK_SESSION_HANDLE,
    _: *mut CK_BYTE,
    _: CK_ULONG,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub5_session_bytes_len_bytes_len(
    _: CK_SESSION_HANDLE,
    _: *mut CK_BYTE,
    _: CK_ULONG,
    _: *mut CK_BYTE,
    _: CK_ULONG,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub3_session_bytes_ulenptr(
    _: CK_SESSION_HANDLE,
    _: *mut CK_BYTE,
    _: *mut CK_ULONG,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub5_session_bytes_len_two_handles(
    _: CK_SESSION_HANDLE,
    _: *mut CK_BYTE,
    _: CK_ULONG,
    _: CK_OBJECT_HANDLE,
    _: CK_OBJECT_HANDLE,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub4_session_attrs_len_handleptr(
    _: CK_SESSION_HANDLE,
    _: *mut CK_ATTRIBUTE,
    _: CK_ULONG,
    _: *mut CK_OBJECT_HANDLE,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub5_session_handle_attrs_len_handleptr(
    _: CK_SESSION_HANDLE,
    _: CK_OBJECT_HANDLE,
    _: *mut CK_ATTRIBUTE,
    _: CK_ULONG,
    _: *mut CK_OBJECT_HANDLE,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub2_session_handle(_: CK_SESSION_HANDLE, _: CK_OBJECT_HANDLE) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub3_session_handle_ulenptr(
    _: CK_SESSION_HANDLE,
    _: CK_OBJECT_HANDLE,
    _: *mut CK_ULONG,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub4_session_handle_attrs_len(
    _: CK_SESSION_HANDLE,
    _: CK_OBJECT_HANDLE,
    _: *mut CK_ATTRIBUTE,
    _: CK_ULONG,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub3_session_mechanism_handle(
    _: CK_SESSION_HANDLE,
    _: *mut CK_MECHANISM,
    _: CK_OBJECT_HANDLE,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub5_session_bytes_len_bytes_ulenptr(
    _: CK_SESSION_HANDLE,
    _: *mut CK_BYTE,
    _: CK_ULONG,
    _: *mut CK_BYTE,
    _: *mut CK_ULONG,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub2_session_mechanism(_: CK_SESSION_HANDLE, _: *mut CK_MECHANISM) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub5_session_mechanism_attrs_len_handleptr(
    _: CK_SESSION_HANDLE,
    _: *mut CK_MECHANISM,
    _: *mut CK_ATTRIBUTE,
    _: CK_ULONG,
    _: *mut CK_OBJECT_HANDLE,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

#[allow(clippy::too_many_arguments)]
pub extern "C" fn stub8_generate_key_pair(
    _: CK_SESSION_HANDLE,
    _: *mut CK_MECHANISM,
    _: *mut CK_ATTRIBUTE,
    _: CK_ULONG,
    _: *mut CK_ATTRIBUTE,
    _: CK_ULONG,
    _: *mut CK_OBJECT_HANDLE,
    _: *mut CK_OBJECT_HANDLE,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub6_wrap_key(
    _: CK_SESSION_HANDLE,
    _: *mut CK_MECHANISM,
    _: CK_OBJECT_HANDLE,
    _: CK_OBJECT_HANDLE,
    _: *mut CK_BYTE,
    _: *mut CK_ULONG,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

#[allow(clippy::too_many_arguments)]
pub extern "C" fn stub8_unwrap_key(
    _: CK_SESSION_HANDLE,
    _: *mut CK_MECHANISM,
    _: CK_OBJECT_HANDLE,
    _: *mut CK_BYTE,
    _: CK_ULONG,
    _: *mut CK_ATTRIBUTE,
    _: CK_ULONG,
    _: *mut CK_OBJECT_HANDLE,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub6_derive_key(
    _: CK_SESSION_HANDLE,
    _: *mut CK_MECHANISM,
    _: CK_OBJECT_HANDLE,
    _: *mut CK_ATTRIBUTE,
    _: CK_ULONG,
    _: *mut CK_OBJECT_HANDLE,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub1_session(_: CK_SESSION_HANDLE) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

pub extern "C" fn stub3_wait_for_slot_event(
    _: CK_FLAGS,
    _: *mut CK_SLOT_ID,
    _: CK_VOID_PTR,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}

/// `C_Verify`'s last parameter is the (fixed-length) signature itself,
/// not a pointer to a returned length -- a different shape from `C_Sign`
/// despite otherwise looking similar.
pub extern "C" fn stub5_verify(
    _: CK_SESSION_HANDLE,
    _: *mut CK_BYTE,
    _: CK_ULONG,
    _: *mut CK_BYTE,
    _: CK_ULONG,
) -> CK_RV {
    CKR_FUNCTION_NOT_SUPPORTED
}
