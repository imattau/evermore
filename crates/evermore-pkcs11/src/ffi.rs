//! Raw PKCS#11 v2.40 C ABI types, struct layouts, and constants --
//! transcribed directly from the authoritative header
//! `/usr/include/p11-kit-1/p11-kit/pkcs11.h` on the VM (not from memory;
//! this ABI has no tolerance for a struct-layout mistake, and p11-kit
//! would either reject the module or crash on a wrong one). Only the base
//! v2.40 function table is implemented -- `C_GetInfo` reports cryptoki
//! version 2.40, so p11-kit never asks for the v3.0/3.2 extensions
//! (message-based bulk encryption etc.) that `homed` has no use for
//! anyway.
//!
//! `CK_ULONG` is `unsigned long`, which is 64-bit on Linux x86_64 (LP64)
//! -- using `std::os::raw::c_ulong` rather than guessing a fixed width
//! is what keeps this correct across platforms.

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::os::raw::{c_uchar, c_ulong, c_void};

pub type CK_BYTE = c_uchar;
pub type CK_BBOOL = c_uchar;
pub type CK_ULONG = c_ulong;
pub type CK_RV = CK_ULONG;
pub type CK_FLAGS = CK_ULONG;
pub type CK_SLOT_ID = CK_ULONG;
pub type CK_SESSION_HANDLE = CK_ULONG;
pub type CK_OBJECT_HANDLE = CK_ULONG;
pub type CK_OBJECT_CLASS = CK_ULONG;
pub type CK_MECHANISM_TYPE = CK_ULONG;
pub type CK_ATTRIBUTE_TYPE = CK_ULONG;
pub type CK_USER_TYPE = CK_ULONG;
pub type CK_STATE = CK_ULONG;
pub type CK_KEY_TYPE = CK_ULONG;
pub type CK_VOID_PTR = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CK_VERSION {
    pub major: CK_BYTE,
    pub minor: CK_BYTE,
}

#[repr(C)]
pub struct CK_INFO {
    pub cryptoki_version: CK_VERSION,
    pub manufacturer_id: [CK_BYTE; 32],
    pub flags: CK_FLAGS,
    pub library_description: [CK_BYTE; 32],
    pub library_version: CK_VERSION,
}

#[repr(C)]
pub struct CK_SLOT_INFO {
    pub slot_description: [CK_BYTE; 64],
    pub manufacturer_id: [CK_BYTE; 32],
    pub flags: CK_FLAGS,
    pub hardware_version: CK_VERSION,
    pub firmware_version: CK_VERSION,
}

#[repr(C)]
pub struct CK_TOKEN_INFO {
    pub label: [CK_BYTE; 32],
    pub manufacturer_id: [CK_BYTE; 32],
    pub model: [CK_BYTE; 16],
    pub serial_number: [CK_BYTE; 16],
    pub flags: CK_FLAGS,
    pub max_session_count: CK_ULONG,
    pub session_count: CK_ULONG,
    pub max_rw_session_count: CK_ULONG,
    pub rw_session_count: CK_ULONG,
    pub max_pin_len: CK_ULONG,
    pub min_pin_len: CK_ULONG,
    pub total_public_memory: CK_ULONG,
    pub free_public_memory: CK_ULONG,
    pub total_private_memory: CK_ULONG,
    pub free_private_memory: CK_ULONG,
    pub hardware_version: CK_VERSION,
    pub firmware_version: CK_VERSION,
    pub utc_time: [CK_BYTE; 16],
}

#[repr(C)]
pub struct CK_SESSION_INFO {
    pub slot_id: CK_SLOT_ID,
    pub state: CK_STATE,
    pub flags: CK_FLAGS,
    pub device_error: CK_ULONG,
}

#[repr(C)]
pub struct CK_ATTRIBUTE {
    pub attribute_type: CK_ATTRIBUTE_TYPE,
    pub value: CK_VOID_PTR,
    pub value_len: CK_ULONG,
}

#[repr(C)]
pub struct CK_MECHANISM {
    pub mechanism: CK_MECHANISM_TYPE,
    pub parameter: CK_VOID_PTR,
    pub parameter_len: CK_ULONG,
}

#[repr(C)]
pub struct CK_MECHANISM_INFO {
    pub min_key_size: CK_ULONG,
    pub max_key_size: CK_ULONG,
    pub flags: CK_FLAGS,
}

#[repr(C)]
pub struct CK_RSA_PKCS_OAEP_PARAMS {
    pub hash_alg: CK_MECHANISM_TYPE,
    pub mgf: CK_ULONG,
    pub source: CK_ULONG,
    pub source_data: CK_VOID_PTR,
    pub source_data_len: CK_ULONG,
}

// --- object classes / key types ---
pub const CKO_PUBLIC_KEY: CK_OBJECT_CLASS = 2;
pub const CKO_PRIVATE_KEY: CK_OBJECT_CLASS = 3;
pub const CKK_RSA: CK_KEY_TYPE = 0;

// --- attribute types ---
pub const CKA_CLASS: CK_ATTRIBUTE_TYPE = 0;
pub const CKA_TOKEN: CK_ATTRIBUTE_TYPE = 1;
pub const CKA_PRIVATE: CK_ATTRIBUTE_TYPE = 2;
pub const CKA_LABEL: CK_ATTRIBUTE_TYPE = 3;
pub const CKA_KEY_TYPE: CK_ATTRIBUTE_TYPE = 0x100;
pub const CKA_ID: CK_ATTRIBUTE_TYPE = 0x102;
pub const CKA_DECRYPT: CK_ATTRIBUTE_TYPE = 0x105;
pub const CKA_MODULUS: CK_ATTRIBUTE_TYPE = 0x120;
pub const CKA_PUBLIC_EXPONENT: CK_ATTRIBUTE_TYPE = 0x122;

// --- mechanisms ---
pub const CKM_RSA_PKCS_OAEP: CK_MECHANISM_TYPE = 9;
pub const CKG_MGF1_SHA1: CK_ULONG = 1;
pub const CKG_MGF1_SHA256: CK_ULONG = 2;
pub const CKZ_DATA_SPECIFIED: CK_ULONG = 1;
pub const CKM_SHA_1: CK_MECHANISM_TYPE = 0x220;
pub const CKM_SHA256: CK_MECHANISM_TYPE = 0x250;

// --- session/slot/user flags and state ---
pub const CKF_TOKEN_PRESENT: CK_FLAGS = 1 << 0;
pub const CKF_RNG: CK_FLAGS = 1 << 0;
pub const CKF_LOGIN_REQUIRED: CK_FLAGS = 1 << 2;
pub const CKF_USER_PIN_INITIALIZED: CK_FLAGS = 1 << 3;
pub const CKF_RW_SESSION: CK_FLAGS = 1 << 1;
pub const CKF_SERIAL_SESSION: CK_FLAGS = 1 << 2;
pub const CKS_RO_PUBLIC_SESSION: CK_STATE = 0;
pub const CKS_RO_USER_FUNCTIONS: CK_STATE = 1;
pub const CKU_USER: CK_USER_TYPE = 1;

// --- return codes ---
pub const CKR_OK: CK_RV = 0;
pub const CKR_GENERAL_ERROR: CK_RV = 5;
pub const CKR_FUNCTION_FAILED: CK_RV = 6;
pub const CKR_ARGUMENTS_BAD: CK_RV = 7;
pub const CKR_DEVICE_ERROR: CK_RV = 0x30;
pub const CKR_PIN_INCORRECT: CK_RV = 0xa0;
pub const CKR_USER_NOT_LOGGED_IN: CK_RV = 0x101;
pub const CKR_BUFFER_TOO_SMALL: CK_RV = 0x150;
pub const CKR_CRYPTOKI_NOT_INITIALIZED: CK_RV = 0x190;
pub const CKR_FUNCTION_NOT_SUPPORTED: CK_RV = 0x54;
pub const CKR_SESSION_HANDLE_INVALID: CK_RV = 0xB3;
pub const CKR_OBJECT_HANDLE_INVALID: CK_RV = 0x82;
pub const CKR_ATTRIBUTE_TYPE_INVALID: CK_RV = 0x12;
pub const CKR_MECHANISM_INVALID: CK_RV = 0x70;
pub const CKR_DATA_LEN_RANGE: CK_RV = 0x21;
pub const CKR_OPERATION_ACTIVE: CK_RV = 0x90;

/// The base v2.40 function table (`CK_FUNCTION_LIST_` in the real header)
/// -- every slot in the exact, authoritative order. All 65 function
/// pointers must be non-null; p11-kit/homed calls through this struct by
/// position, not by name.
#[repr(C)]
pub struct CK_FUNCTION_LIST {
    pub version: CK_VERSION,
    pub C_Initialize: extern "C" fn(CK_VOID_PTR) -> CK_RV,
    pub C_Finalize: extern "C" fn(CK_VOID_PTR) -> CK_RV,
    pub C_GetInfo: extern "C" fn(*mut CK_INFO) -> CK_RV,
    pub C_GetFunctionList: extern "C" fn(*mut *mut CK_FUNCTION_LIST) -> CK_RV,
    pub C_GetSlotList: extern "C" fn(CK_BBOOL, *mut CK_SLOT_ID, *mut CK_ULONG) -> CK_RV,
    pub C_GetSlotInfo: extern "C" fn(CK_SLOT_ID, *mut CK_SLOT_INFO) -> CK_RV,
    pub C_GetTokenInfo: extern "C" fn(CK_SLOT_ID, *mut CK_TOKEN_INFO) -> CK_RV,
    pub C_GetMechanismList:
        extern "C" fn(CK_SLOT_ID, *mut CK_MECHANISM_TYPE, *mut CK_ULONG) -> CK_RV,
    pub C_GetMechanismInfo:
        extern "C" fn(CK_SLOT_ID, CK_MECHANISM_TYPE, *mut CK_MECHANISM_INFO) -> CK_RV,
    pub C_InitToken: extern "C" fn(CK_SLOT_ID, *mut CK_BYTE, CK_ULONG, *mut CK_BYTE) -> CK_RV,
    pub C_InitPIN: extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, CK_ULONG) -> CK_RV,
    pub C_SetPIN:
        extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, CK_ULONG, *mut CK_BYTE, CK_ULONG) -> CK_RV,
    pub C_OpenSession: extern "C" fn(
        CK_SLOT_ID,
        CK_FLAGS,
        CK_VOID_PTR,
        CK_VOID_PTR,
        *mut CK_SESSION_HANDLE,
    ) -> CK_RV,
    pub C_CloseSession: extern "C" fn(CK_SESSION_HANDLE) -> CK_RV,
    pub C_CloseAllSessions: extern "C" fn(CK_SLOT_ID) -> CK_RV,
    pub C_GetSessionInfo: extern "C" fn(CK_SESSION_HANDLE, *mut CK_SESSION_INFO) -> CK_RV,
    pub C_GetOperationState: extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, *mut CK_ULONG) -> CK_RV,
    pub C_SetOperationState: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        CK_OBJECT_HANDLE,
        CK_OBJECT_HANDLE,
    ) -> CK_RV,
    pub C_Login: extern "C" fn(CK_SESSION_HANDLE, CK_USER_TYPE, *mut CK_BYTE, CK_ULONG) -> CK_RV,
    pub C_Logout: extern "C" fn(CK_SESSION_HANDLE) -> CK_RV,
    pub C_CreateObject: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_ATTRIBUTE,
        CK_ULONG,
        *mut CK_OBJECT_HANDLE,
    ) -> CK_RV,
    pub C_CopyObject: extern "C" fn(
        CK_SESSION_HANDLE,
        CK_OBJECT_HANDLE,
        *mut CK_ATTRIBUTE,
        CK_ULONG,
        *mut CK_OBJECT_HANDLE,
    ) -> CK_RV,
    pub C_DestroyObject: extern "C" fn(CK_SESSION_HANDLE, CK_OBJECT_HANDLE) -> CK_RV,
    pub C_GetObjectSize: extern "C" fn(CK_SESSION_HANDLE, CK_OBJECT_HANDLE, *mut CK_ULONG) -> CK_RV,
    pub C_GetAttributeValue:
        extern "C" fn(CK_SESSION_HANDLE, CK_OBJECT_HANDLE, *mut CK_ATTRIBUTE, CK_ULONG) -> CK_RV,
    pub C_SetAttributeValue:
        extern "C" fn(CK_SESSION_HANDLE, CK_OBJECT_HANDLE, *mut CK_ATTRIBUTE, CK_ULONG) -> CK_RV,
    pub C_FindObjectsInit: extern "C" fn(CK_SESSION_HANDLE, *mut CK_ATTRIBUTE, CK_ULONG) -> CK_RV,
    pub C_FindObjects:
        extern "C" fn(CK_SESSION_HANDLE, *mut CK_OBJECT_HANDLE, CK_ULONG, *mut CK_ULONG) -> CK_RV,
    pub C_FindObjectsFinal: extern "C" fn(CK_SESSION_HANDLE) -> CK_RV,
    pub C_EncryptInit:
        extern "C" fn(CK_SESSION_HANDLE, *mut CK_MECHANISM, CK_OBJECT_HANDLE) -> CK_RV,
    pub C_Encrypt: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        *mut CK_BYTE,
        *mut CK_ULONG,
    ) -> CK_RV,
    pub C_EncryptUpdate: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        *mut CK_BYTE,
        *mut CK_ULONG,
    ) -> CK_RV,
    pub C_EncryptFinal: extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, *mut CK_ULONG) -> CK_RV,
    pub C_DecryptInit:
        extern "C" fn(CK_SESSION_HANDLE, *mut CK_MECHANISM, CK_OBJECT_HANDLE) -> CK_RV,
    pub C_Decrypt: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        *mut CK_BYTE,
        *mut CK_ULONG,
    ) -> CK_RV,
    pub C_DecryptUpdate: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        *mut CK_BYTE,
        *mut CK_ULONG,
    ) -> CK_RV,
    pub C_DecryptFinal: extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, *mut CK_ULONG) -> CK_RV,
    pub C_DigestInit: extern "C" fn(CK_SESSION_HANDLE, *mut CK_MECHANISM) -> CK_RV,
    pub C_Digest: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        *mut CK_BYTE,
        *mut CK_ULONG,
    ) -> CK_RV,
    pub C_DigestUpdate: extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, CK_ULONG) -> CK_RV,
    pub C_DigestKey: extern "C" fn(CK_SESSION_HANDLE, CK_OBJECT_HANDLE) -> CK_RV,
    pub C_DigestFinal: extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, *mut CK_ULONG) -> CK_RV,
    pub C_SignInit: extern "C" fn(CK_SESSION_HANDLE, *mut CK_MECHANISM, CK_OBJECT_HANDLE) -> CK_RV,
    pub C_Sign: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        *mut CK_BYTE,
        *mut CK_ULONG,
    ) -> CK_RV,
    pub C_SignUpdate: extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, CK_ULONG) -> CK_RV,
    pub C_SignFinal: extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, *mut CK_ULONG) -> CK_RV,
    pub C_SignRecoverInit:
        extern "C" fn(CK_SESSION_HANDLE, *mut CK_MECHANISM, CK_OBJECT_HANDLE) -> CK_RV,
    pub C_SignRecover: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        *mut CK_BYTE,
        *mut CK_ULONG,
    ) -> CK_RV,
    pub C_VerifyInit:
        extern "C" fn(CK_SESSION_HANDLE, *mut CK_MECHANISM, CK_OBJECT_HANDLE) -> CK_RV,
    pub C_Verify:
        extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, CK_ULONG, *mut CK_BYTE, CK_ULONG) -> CK_RV,
    pub C_VerifyUpdate: extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, CK_ULONG) -> CK_RV,
    pub C_VerifyFinal: extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, CK_ULONG) -> CK_RV,
    pub C_VerifyRecoverInit:
        extern "C" fn(CK_SESSION_HANDLE, *mut CK_MECHANISM, CK_OBJECT_HANDLE) -> CK_RV,
    pub C_VerifyRecover: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        *mut CK_BYTE,
        *mut CK_ULONG,
    ) -> CK_RV,
    pub C_DigestEncryptUpdate: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        *mut CK_BYTE,
        *mut CK_ULONG,
    ) -> CK_RV,
    pub C_DecryptDigestUpdate: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        *mut CK_BYTE,
        *mut CK_ULONG,
    ) -> CK_RV,
    pub C_SignEncryptUpdate: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        *mut CK_BYTE,
        *mut CK_ULONG,
    ) -> CK_RV,
    pub C_DecryptVerifyUpdate: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        *mut CK_BYTE,
        *mut CK_ULONG,
    ) -> CK_RV,
    pub C_GenerateKey: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_MECHANISM,
        *mut CK_ATTRIBUTE,
        CK_ULONG,
        *mut CK_OBJECT_HANDLE,
    ) -> CK_RV,
    pub C_GenerateKeyPair: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_MECHANISM,
        *mut CK_ATTRIBUTE,
        CK_ULONG,
        *mut CK_ATTRIBUTE,
        CK_ULONG,
        *mut CK_OBJECT_HANDLE,
        *mut CK_OBJECT_HANDLE,
    ) -> CK_RV,
    pub C_WrapKey: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_MECHANISM,
        CK_OBJECT_HANDLE,
        CK_OBJECT_HANDLE,
        *mut CK_BYTE,
        *mut CK_ULONG,
    ) -> CK_RV,
    pub C_UnwrapKey: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_MECHANISM,
        CK_OBJECT_HANDLE,
        *mut CK_BYTE,
        CK_ULONG,
        *mut CK_ATTRIBUTE,
        CK_ULONG,
        *mut CK_OBJECT_HANDLE,
    ) -> CK_RV,
    pub C_DeriveKey: extern "C" fn(
        CK_SESSION_HANDLE,
        *mut CK_MECHANISM,
        CK_OBJECT_HANDLE,
        *mut CK_ATTRIBUTE,
        CK_ULONG,
        *mut CK_OBJECT_HANDLE,
    ) -> CK_RV,
    pub C_SeedRandom: extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, CK_ULONG) -> CK_RV,
    pub C_GenerateRandom: extern "C" fn(CK_SESSION_HANDLE, *mut CK_BYTE, CK_ULONG) -> CK_RV,
    pub C_GetFunctionStatus: extern "C" fn(CK_SESSION_HANDLE) -> CK_RV,
    pub C_CancelFunction: extern "C" fn(CK_SESSION_HANDLE) -> CK_RV,
    pub C_WaitForSlotEvent: extern "C" fn(CK_FLAGS, *mut CK_SLOT_ID, CK_VOID_PTR) -> CK_RV,
}
