//! Vault-backed encryption for provider API keys.
//!
//! Uses the OS keychain (macOS Keychain, Windows Credential Manager,
//! Linux kernel keyutils) to store a master encryption key. Provider
//! API keys are encrypted with ChaCha20-Poly1305 using a key derived
//! from the master via HKDF-SHA256.
//!
//! When the OS keychain is unavailable, the vault holds no master key
//! and refuses to persist secrets on disk. Callers fall back to reading
//! the key from an environment variable at runtime (see `env_key_for`).
//! Plaintext key material is never written to disk and never logged.

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use hkdf::Hkdf;
use sha2::Sha256;
use thiserror::Error;

const KEYCHAIN_SERVICE: &str = "swarm-manager";
const KEYCHAIN_USER: &str = "master-key";
const KEY_DERIVATION_CONTEXT: &[u8] = b"SwarmManager-v1-ProviderKey";
const NONCE_SIZE: usize = 12;

/// Prefix for the per-provider environment-variable fallback. The full
/// variable name is `SWARM_PROVIDER_KEY_<UPPERCASE_ID>` where the id has
/// every non-alphanumeric character replaced with `_`.
pub const ENV_KEY_PREFIX: &str = "SWARM_PROVIDER_KEY_";

/// Version tag prepended to encrypted data to distinguish it from any
/// other stored value.
const ENCRYPTED_VERSION_TAG: u8 = 0x01;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("encryption failed: {0}")]
    EncryptionFailed(String),
    #[error("decryption failed: {0}")]
    DecryptionFailed(String),
    #[error("keychain unavailable: {0}")]
    KeychainUnavailable(String),
    #[error("key derivation failed")]
    KeyDerivationFailed,
    #[error("hex decode error: {0}")]
    HexDecode(String),
}

/// Build the environment-variable name that holds a provider's key when
/// no OS keychain is available.
///
/// Environment variable names cannot contain `-`, so every character
/// outside `[A-Za-z0-9]` is normalized to `_` and the result is
/// uppercased (provider ids are v4 UUIDs).
pub fn env_key_for(id: &str) -> String {
    let normalized: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{ENV_KEY_PREFIX}{}", normalized.to_uppercase())
}

/// Manages a master encryption key stored in the OS keychain.
///
/// When the keychain is unavailable the vault carries no master key and
/// will not persist secrets; callers use the environment-variable
/// fallback instead.
pub struct KeychainVault {
    master_key: Option<[u8; 32]>,
}

impl Default for KeychainVault {
    fn default() -> Self {
        Self::new()
    }
}

impl KeychainVault {
    /// Initialize the vault by loading or creating a master key in the OS
    /// keychain. If the keychain is unavailable the vault holds no master
    /// key — secrets are not persisted and the env-var fallback applies.
    pub fn new() -> Self {
        match Self::load_or_create_master_key() {
            Ok(key) => Self {
                master_key: Some(key),
            },
            Err(_) => Self { master_key: None },
        }
    }

    /// Create a vault with no master key (env-var fallback path).
    pub fn without_keychain() -> Self {
        Self { master_key: None }
    }

    /// Create a vault with a specific master key (for testing).
    #[cfg(test)]
    pub fn with_key(key: [u8; 32]) -> Self {
        Self {
            master_key: Some(key),
        }
    }

    /// Returns true if at-rest encryption is available (master key present).
    pub fn is_encrypted(&self) -> bool {
        self.master_key.is_some()
    }

    /// Encrypt an API key string. Returns a hex-encoded string suitable
    /// for storage in SQLite TEXT columns.
    ///
    /// Format: hex([0x01 version tag][12-byte nonce][ciphertext])
    ///
    /// Errors with `KeychainUnavailable` if no master key is present — the
    /// vault never returns plaintext for at-rest storage.
    pub fn encrypt(&self, plaintext: &str) -> Result<String, CryptoError> {
        let master = self.master_key.ok_or_else(|| {
            CryptoError::KeychainUnavailable("no master key; cannot encrypt for storage".into())
        })?;

        let derived = Self::derive_key(&master, KEY_DERIVATION_CONTEXT)
            .ok_or(CryptoError::KeyDerivationFailed)?;
        let cipher = ChaCha20Poly1305::new((&derived).into());

        let mut nonce_bytes = [0u8; NONCE_SIZE];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;

        let mut result = Vec::with_capacity(1 + NONCE_SIZE + ciphertext.len());
        result.push(ENCRYPTED_VERSION_TAG);
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);

        Ok(hex::encode(result))
    }

    /// Decrypt a hex-encoded encrypted value produced by `encrypt`.
    pub fn decrypt(&self, data: &str) -> Result<String, CryptoError> {
        if data.is_empty() {
            return Ok(String::new());
        }

        let bytes = hex::decode(data).map_err(|e| CryptoError::HexDecode(e.to_string()))?;

        if bytes.is_empty() {
            return Ok(String::new());
        }

        if bytes[0] != ENCRYPTED_VERSION_TAG {
            return Err(CryptoError::DecryptionFailed(
                "stored value is not in the expected encrypted format".into(),
            ));
        }

        let master = self.master_key.ok_or_else(|| {
            CryptoError::DecryptionFailed("data is encrypted but no master key is available".into())
        })?;

        let min_len = 1 + NONCE_SIZE + 1; // tag + nonce + at least 1 byte ciphertext
        if bytes.len() < min_len {
            return Err(CryptoError::DecryptionFailed("data too short".into()));
        }

        let nonce = Nonce::from_slice(&bytes[1..1 + NONCE_SIZE]);
        let ciphertext = &bytes[1 + NONCE_SIZE..];

        let derived = Self::derive_key(&master, KEY_DERIVATION_CONTEXT)
            .ok_or(CryptoError::KeyDerivationFailed)?;
        let cipher = ChaCha20Poly1305::new((&derived).into());

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))?;

        String::from_utf8(plaintext).map_err(|e| CryptoError::DecryptionFailed(e.to_string()))
    }

    /// Check if a stored value is encrypted (has the version tag after hex-decode).
    pub fn is_value_encrypted(data: &str) -> bool {
        if let Ok(bytes) = hex::decode(data) {
            !bytes.is_empty() && bytes[0] == ENCRYPTED_VERSION_TAG
        } else {
            false
        }
    }

    /// Load the master key from the keychain, or generate and store one.
    fn load_or_create_master_key() -> Result<[u8; 32], CryptoError> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER)
            .map_err(|e| CryptoError::KeychainUnavailable(e.to_string()))?;

        match entry.get_password() {
            Ok(hex_key) => Self::parse_master_key_hex(&hex_key).map_err(|e| {
                CryptoError::KeychainUnavailable(format!("stored key is invalid: {e}"))
            }),
            Err(keyring::Error::NoEntry) => {
                let mut key = [0u8; 32];
                rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key);
                let hex_key = hex::encode(key);
                entry
                    .set_password(&hex_key)
                    .map_err(|e| CryptoError::KeychainUnavailable(e.to_string()))?;
                Ok(key)
            }
            Err(e) => Err(CryptoError::KeychainUnavailable(e.to_string())),
        }
    }

    fn parse_master_key_hex(hex_key: &str) -> Result<[u8; 32], CryptoError> {
        let bytes = hex::decode(hex_key).map_err(|e| CryptoError::HexDecode(e.to_string()))?;
        if bytes.len() != 32 {
            return Err(CryptoError::KeychainUnavailable(format!(
                "stored key has wrong length: {} (expected 32)",
                bytes.len()
            )));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Ok(key)
    }

    /// Derive a purpose-specific key from the master using HKDF-SHA256.
    fn derive_key(master: &[u8; 32], context: &[u8]) -> Option<[u8; 32]> {
        let hkdf = Hkdf::<Sha256>::new(None, master);
        let mut derived = [0u8; 32];
        hkdf.expand(context, &mut derived).ok()?;
        Some(derived)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = i as u8;
        }
        key
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let vault = KeychainVault::with_key(test_key());
        let plaintext = "sk-test-api-key-12345";

        let encrypted = vault.encrypt(plaintext).unwrap();
        assert_ne!(encrypted, plaintext);

        let decrypted = vault.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypted_value_has_version_tag() {
        let vault = KeychainVault::with_key(test_key());
        let encrypted = vault.encrypt("my-secret").unwrap();
        assert!(KeychainVault::is_value_encrypted(&encrypted));
    }

    #[test]
    fn plaintext_not_detected_as_encrypted() {
        assert!(!KeychainVault::is_value_encrypted("sk-plain-api-key"));
        assert!(!KeychainVault::is_value_encrypted(""));
    }

    #[test]
    fn without_keychain_refuses_to_encrypt() {
        // Born-clean rule: never store plaintext on disk. A vault with no
        // master key must error rather than return plaintext.
        let vault = KeychainVault::without_keychain();
        let result = vault.encrypt("sk-secret");
        assert!(matches!(result, Err(CryptoError::KeychainUnavailable(_))));
    }

    #[test]
    fn decrypt_empty_string() {
        let vault = KeychainVault::with_key(test_key());
        assert_eq!(vault.decrypt("").unwrap(), "");
    }

    #[test]
    fn encrypted_without_master_key_fails() {
        let vault_with_key = KeychainVault::with_key(test_key());
        let encrypted = vault_with_key.encrypt("secret").unwrap();

        let vault_without = KeychainVault::without_keychain();
        assert!(vault_without.decrypt(&encrypted).is_err());
    }

    #[test]
    fn wrong_master_key_fails_to_decrypt() {
        let vault = KeychainVault::with_key(test_key());
        let encrypted = vault.encrypt("secret").unwrap();

        let other_key = {
            let mut k = [0u8; 32];
            for (i, byte) in k.iter_mut().enumerate() {
                *byte = (i as u8).wrapping_add(0x55);
            }
            k
        };
        let other_vault = KeychainVault::with_key(other_key);
        assert!(other_vault.decrypt(&encrypted).is_err());
    }

    #[test]
    fn different_encryptions_produce_different_output() {
        let vault = KeychainVault::with_key(test_key());
        let plaintext = "same-api-key";

        let encrypted1 = vault.encrypt(plaintext).unwrap();
        let encrypted2 = vault.encrypt(plaintext).unwrap();

        assert_ne!(encrypted1, encrypted2);
        assert_eq!(vault.decrypt(&encrypted1).unwrap(), plaintext);
        assert_eq!(vault.decrypt(&encrypted2).unwrap(), plaintext);
    }

    #[test]
    fn parse_master_key_hex_requires_32_bytes() {
        let parsed = KeychainVault::parse_master_key_hex(&hex::encode(test_key())).unwrap();
        assert_eq!(parsed, test_key());

        let err = KeychainVault::parse_master_key_hex("abcd").unwrap_err();
        assert!(matches!(err, CryptoError::KeychainUnavailable(_)));
    }

    #[test]
    fn env_key_normalizes_uuid_dashes() {
        let id = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
        let var = env_key_for(id);
        assert_eq!(
            var,
            "SWARM_PROVIDER_KEY_3F2504E0_4F89_41D3_9A0C_0305E82C3301"
        );
        assert!(!var.contains('-'));
    }
}
