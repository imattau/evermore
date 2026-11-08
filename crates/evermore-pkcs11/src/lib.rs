//! `evermore-pkcs11`: a PKCS#11 v2.40 provider, loaded by `systemd-homed`
//! (via `p11-kit`) to unlock a home directory enrolled with
//! `--pkcs11-token-uri=`.
//!
//! **What this increment proves, verified against real `homed` on the
//! VM, not assumed:** the exact JSON identity-record schema homed uses
//! for PKCS#11 enrollment (`pkcs11TokenUri`, `privileged.pkcs11EncryptedKey`
//! with `uri`/`data`/`hashedPassword`/`padding`, `secret.tokenPin`), the
//! mechanism it picks (`CKM_RSA_PKCS_OAEP` with SHA-1/MGF1-SHA1), and the
//! exact sequence of PKCS#11 calls it makes (enrollment reads the public
//! key's `CKA_MODULUS`/`CKA_PUBLIC_EXPONENT`, logs into the token to
//! fetch random bytes via `C_GenerateRandom`; unlock logs in with the PIN
//! and calls `C_DecryptInit`/`C_Decrypt` against the private key).
//!
//! `C_Decrypt` now also gates on a live NIP-46 approval round-trip with
//! a paired remote signer before any RSA-OAEP math runs -- see
//! `nip46_gate`'s module doc for the protocol and `functions::c_decrypt`
//! for where it's called. See PROJECT_IDEA.md's Phase 5.5.

mod ffi;
mod functions;
mod nip46_gate;
mod state;
mod stubs;

use ffi::*;

static FUNCTION_LIST: CK_FUNCTION_LIST = CK_FUNCTION_LIST {
    version: CK_VERSION {
        major: 2,
        minor: 40,
    },
    C_Initialize: functions::c_initialize,
    C_Finalize: functions::c_finalize,
    C_GetInfo: functions::c_get_info,
    C_GetFunctionList: c_get_function_list,
    C_GetSlotList: functions::c_get_slot_list,
    C_GetSlotInfo: functions::c_get_slot_info,
    C_GetTokenInfo: functions::c_get_token_info,
    C_GetMechanismList: functions::c_get_mechanism_list,
    C_GetMechanismInfo: functions::c_get_mechanism_info,
    C_InitToken: stubs::stub4_slot_bytes,
    C_InitPIN: stubs::stub3_session_bytes_len,
    C_SetPIN: stubs::stub5_session_bytes_len_bytes_len,
    C_OpenSession: functions::c_open_session,
    C_CloseSession: functions::c_close_session,
    C_CloseAllSessions: functions::c_close_all_sessions,
    C_GetSessionInfo: functions::c_get_session_info,
    C_GetOperationState: stubs::stub3_session_bytes_ulenptr,
    C_SetOperationState: stubs::stub5_session_bytes_len_two_handles,
    C_Login: functions::c_login,
    C_Logout: functions::c_logout,
    C_CreateObject: stubs::stub4_session_attrs_len_handleptr,
    C_CopyObject: stubs::stub5_session_handle_attrs_len_handleptr,
    C_DestroyObject: stubs::stub2_session_handle,
    C_GetObjectSize: stubs::stub3_session_handle_ulenptr,
    C_GetAttributeValue: functions::c_get_attribute_value,
    C_SetAttributeValue: stubs::stub4_session_handle_attrs_len,
    C_FindObjectsInit: functions::c_find_objects_init,
    C_FindObjects: functions::c_find_objects,
    C_FindObjectsFinal: functions::c_find_objects_final,
    C_EncryptInit: stubs::stub3_session_mechanism_handle,
    C_Encrypt: stubs::stub5_session_bytes_len_bytes_ulenptr,
    C_EncryptUpdate: stubs::stub5_session_bytes_len_bytes_ulenptr,
    C_EncryptFinal: stubs::stub3_session_bytes_ulenptr,
    C_DecryptInit: functions::c_decrypt_init,
    C_Decrypt: functions::c_decrypt,
    C_DecryptUpdate: stubs::stub5_session_bytes_len_bytes_ulenptr,
    C_DecryptFinal: stubs::stub3_session_bytes_ulenptr,
    C_DigestInit: stubs::stub2_session_mechanism,
    C_Digest: stubs::stub5_session_bytes_len_bytes_ulenptr,
    C_DigestUpdate: stubs::stub3_session_bytes_len,
    C_DigestKey: stubs::stub2_session_handle,
    C_DigestFinal: stubs::stub3_session_bytes_ulenptr,
    C_SignInit: stubs::stub3_session_mechanism_handle,
    C_Sign: stubs::stub5_session_bytes_len_bytes_ulenptr,
    C_SignUpdate: stubs::stub3_session_bytes_len,
    C_SignFinal: stubs::stub3_session_bytes_ulenptr,
    C_SignRecoverInit: stubs::stub3_session_mechanism_handle,
    C_SignRecover: stubs::stub5_session_bytes_len_bytes_ulenptr,
    C_VerifyInit: stubs::stub3_session_mechanism_handle,
    C_Verify: stubs::stub5_verify,
    C_VerifyUpdate: stubs::stub3_session_bytes_len,
    C_VerifyFinal: stubs::stub3_session_bytes_len,
    C_VerifyRecoverInit: stubs::stub3_session_mechanism_handle,
    C_VerifyRecover: stubs::stub5_session_bytes_len_bytes_ulenptr,
    C_DigestEncryptUpdate: stubs::stub5_session_bytes_len_bytes_ulenptr,
    C_DecryptDigestUpdate: stubs::stub5_session_bytes_len_bytes_ulenptr,
    C_SignEncryptUpdate: stubs::stub5_session_bytes_len_bytes_ulenptr,
    C_DecryptVerifyUpdate: stubs::stub5_session_bytes_len_bytes_ulenptr,
    C_GenerateKey: stubs::stub5_session_mechanism_attrs_len_handleptr,
    C_GenerateKeyPair: stubs::stub8_generate_key_pair,
    C_WrapKey: stubs::stub6_wrap_key,
    C_UnwrapKey: stubs::stub8_unwrap_key,
    C_DeriveKey: stubs::stub6_derive_key,
    C_SeedRandom: functions::c_seed_random,
    C_GenerateRandom: functions::c_generate_random,
    C_GetFunctionStatus: stubs::stub1_session,
    C_CancelFunction: stubs::stub1_session,
    C_WaitForSlotEvent: stubs::stub3_wait_for_slot_event,
};

extern "C" fn c_get_function_list(list: *mut *mut CK_FUNCTION_LIST) -> CK_RV {
    if list.is_null() {
        return CKR_ARGUMENTS_BAD;
    }
    unsafe {
        *list = std::ptr::addr_of!(FUNCTION_LIST) as *mut CK_FUNCTION_LIST;
    }
    CKR_OK
}

/// The one symbol p11-kit actually looks for when loading a PKCS#11
/// module by path -- everything else is reached through the returned
/// function table.
#[unsafe(no_mangle)]
pub extern "C" fn C_GetFunctionList(list: *mut *mut CK_FUNCTION_LIST) -> CK_RV {
    c_get_function_list(list)
}
