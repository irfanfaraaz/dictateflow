//! macOS Keychain wrapper for storing API keys (BYOK secrets).
//!
//! All secrets live in a generic password keychain item under the
//! service `com.wispertype.app`. The frontend never sees the actual
//! key — it only sees `has_api_key: bool` derived from presence here.
//!
//! # Why we route through Keychain instead of a settings file
//!
//! - OS-mediated encryption at rest (each app's secrets are isolated).
//! - Standard macOS user-prompt UX on first access.
//! - Survives app uninstall/reinstall when the user wants it to.
//!
//! # `errSecItemNotFound` (-25300)
//!
//! Apple's Security framework treats "no item exists" as an error code
//! rather than `Ok(None)`. We translate it into idiomatic `Option`/`()`
//! shapes here so callers can match on real failures.

use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use thiserror::Error;

/// Service name used for all Wispertype keychain entries.
const SERVICE: &str = "com.wispertype.app";

/// Account key under which the ElevenLabs API key is stored.
pub const ELEVENLABS_KEY: &str = "elevenlabs_api_key";

/// `errSecItemNotFound` — Apple's "no such keychain item" status.
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

#[derive(Debug, Error)]
pub enum KeychainError {
    #[error("keychain access error: {0}")]
    SecurityFramework(#[from] security_framework::base::Error),

    #[error("invalid utf-8 in stored secret")]
    InvalidUtf8,
}

/// Store `value` under `key` (overwrites any existing entry).
pub fn set(key: &str, value: &str) -> Result<(), KeychainError> {
    set_generic_password(SERVICE, key, value.as_bytes())?;
    Ok(())
}

/// Retrieve the secret under `key`, or `Ok(None)` if no entry exists.
pub fn get(key: &str) -> Result<Option<String>, KeychainError> {
    match get_generic_password(SERVICE, key) {
        Ok(bytes) => {
            let s = String::from_utf8(bytes).map_err(|_| KeychainError::InvalidUtf8)?;
            Ok(Some(s))
        }
        Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        Err(e) => Err(KeychainError::SecurityFramework(e)),
    }
}

/// Remove the entry under `key`. Idempotent: missing entries return `Ok(())`.
pub fn delete(key: &str) -> Result<(), KeychainError> {
    match delete_generic_password(SERVICE, key) {
        Ok(()) => Ok(()),
        Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        Err(e) => Err(KeychainError::SecurityFramework(e)),
    }
}

/// Convenience: presence-only check (frontend uses this to render
/// "API key set" without ever reading the secret).
pub fn has(key: &str) -> Result<bool, KeychainError> {
    Ok(get(key)?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RAII guard so the test always cleans up even on panic.
    struct Cleanup<'a>(&'a str);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = delete(self.0);
        }
    }

    #[test]
    #[ignore = "writes to real keychain"]
    fn set_get_delete_round_trip() {
        let key = "wispertype-test-key-xyz";
        let value = "secret-value";
        let _guard = Cleanup(key);

        // Pre-clean in case a prior failed run left an entry.
        delete(key).expect("pre-clean ok");

        assert_eq!(get(key).expect("get-empty"), None);
        assert!(!has(key).expect("has-empty"));

        set(key, value).expect("set");
        assert_eq!(get(key).expect("get").as_deref(), Some(value));
        assert!(has(key).expect("has"));

        // Overwrite path
        set(key, "v2").expect("set-overwrite");
        assert_eq!(get(key).expect("get-2").as_deref(), Some("v2"));

        delete(key).expect("delete");
        assert_eq!(get(key).expect("get-after-delete"), None);

        // Idempotent delete
        delete(key).expect("delete-again-ok");
    }
}
