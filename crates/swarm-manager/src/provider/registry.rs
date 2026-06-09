//! Provider registry — persistent storage of LLM provider configurations.
//!
//! Users can add multiple provider instances (e.g. multiple local
//! endpoints, or the same cloud provider with different keys). Agents
//! reference these by id.
//!
//! API keys are encrypted at rest using ChaCha20-Poly1305 with a master
//! key from the OS keychain. When no keychain is available, keys are NOT
//! persisted on disk; instead they are resolved at read time from the
//! `SWARM_PROVIDER_KEY_<ID>` environment variable. Key material is never
//! written to disk in plaintext and never logged.

use crate::provider::crypto::{env_key_for, KeychainVault};
use crate::provider::ProviderType;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use uuid::Uuid;

/// Three-way view of an API-key's storage health, derived from
/// `ProviderConfig` after the registry has run decryption / env lookup.
///
/// - `Absent` — no key available (no ciphertext and no env var).
/// - `Healthy` — a usable plaintext key was resolved.
/// - `Stranded` — ciphertext is stored on disk but could not be decrypted
///   (e.g. the master key rotated). Surfaces as a loud error path, never a
///   silent fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyStatus {
    Absent,
    Healthy,
    Stranded,
}

/// A configured provider instance.
///
/// Unlike `ProviderType` (an enum of provider kinds), `ProviderConfig`
/// represents a specific user-configured instance with credentials and
/// settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Unique identifier for this configuration.
    pub id: String,
    /// Display name (e.g. "Work Provider", "Home Endpoint").
    pub name: String,
    /// Provider type (openai, anthropic, etc.).
    pub provider_type: ProviderType,
    /// Custom endpoint URL (optional, uses default if not set).
    pub endpoint: Option<String>,
    /// API key (decrypted / resolved in memory; never serialized to disk here).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Available models for this provider.
    #[serde(default)]
    pub models: Vec<String>,
    /// Whether this is a local provider (no cloud).
    #[serde(default)]
    pub is_local: bool,
    /// Whether this provider is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Created timestamp (unix seconds).
    #[serde(default)]
    pub created_at: i64,
    /// True when a non-empty ciphertext is stored in the underlying row.
    /// Populated by `list()`; meaningless on freshly-constructed configs.
    /// Kept out of the serde wire format.
    #[serde(default, skip_serializing)]
    pub has_encrypted_key: bool,
}

fn default_true() -> bool {
    true
}

impl ProviderConfig {
    /// Create a new provider config with a generated id.
    pub fn new(
        name: String,
        provider_type: ProviderType,
        endpoint: Option<String>,
        api_key: Option<String>,
    ) -> Self {
        let is_local = matches!(
            provider_type,
            ProviderType::Ollama | ProviderType::LMStudio | ProviderType::MLX
        );
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            provider_type,
            endpoint,
            api_key,
            models: vec![],
            is_local,
            enabled: true,
            created_at: chrono::Utc::now().timestamp(),
            has_encrypted_key: false,
        }
    }

    /// Get the effective endpoint URL.
    pub fn effective_endpoint(&self) -> String {
        self.endpoint.clone().unwrap_or_else(|| {
            self.provider_type
                .default_endpoint()
                .unwrap_or("")
                .to_string()
        })
    }

    /// Three-way view of this row's API-key storage health.
    ///
    /// `Absent` when no key is available, `Healthy` when a usable key was
    /// resolved (decrypted ciphertext OR env-var fallback), and `Stranded`
    /// when ciphertext exists on disk but could not be decrypted.
    pub fn key_status(&self) -> KeyStatus {
        match (self.has_encrypted_key, self.api_key.is_some()) {
            (false, false) => KeyStatus::Absent,
            (false, true) => KeyStatus::Healthy, // env-var fallback resolved
            (true, true) => KeyStatus::Healthy,
            (true, false) => KeyStatus::Stranded,
        }
    }
}

/// Provider registry — stores and manages provider configurations.
///
/// API keys are encrypted at rest with a master key from the OS keychain.
/// When the keychain is unavailable, keys are not persisted and are
/// resolved from the environment at read time.
pub struct ProviderRegistry {
    db_path: PathBuf,
    lock: Mutex<()>,
    vault: KeychainVault,
}

impl ProviderRegistry {
    /// Open or create the registry database.
    pub fn open(data_dir: &PathBuf) -> anyhow::Result<Self> {
        Self::open_with_vault(data_dir, KeychainVault::new())
    }

    /// Open the registry with a specific vault.
    pub fn open_with_vault(data_dir: &PathBuf, vault: KeychainVault) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let db_path = data_dir.join("providers.db");

        let conn = Connection::open(&db_path)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS providers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                provider_type TEXT NOT NULL,
                endpoint TEXT,
                api_key_encrypted TEXT,
                models TEXT DEFAULT '[]',
                is_local INTEGER DEFAULT 0,
                enabled INTEGER DEFAULT 1,
                created_at INTEGER DEFAULT 0
            )",
            [],
        )?;
        drop(conn);

        Ok(Self {
            db_path,
            lock: Mutex::new(()),
            vault,
        })
    }

    fn connect(&self) -> anyhow::Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    /// Encrypt an API key for storage.
    ///
    /// When no keychain is available the key cannot be persisted safely
    /// (plaintext on disk is forbidden). In that case this returns `None`
    /// and logs loudly to stderr so the failure is never silent — the key
    /// must be supplied at runtime via `SWARM_PROVIDER_KEY_<ID>`.
    fn encrypt_key(&self, id: &str, api_key: &Option<String>) -> anyhow::Result<Option<String>> {
        match api_key {
            Some(key) if !key.is_empty() => {
                if !self.vault.is_encrypted() {
                    eprintln!(
                        "[swarm] WARNING: no OS keychain available; API key for provider \
                         '{id}' will NOT be persisted. Supply it at runtime via the \
                         '{}' environment variable.",
                        env_key_for(id)
                    );
                    return Ok(None);
                }
                let encrypted = self
                    .vault
                    .encrypt(key)
                    .map_err(|e| anyhow::anyhow!("encryption failed: {}", e))?;
                Ok(Some(encrypted))
            }
            _ => Ok(None),
        }
    }

    /// Decrypt an API key from storage, returning `None` on failure.
    fn decrypt_key(&self, encrypted: &Option<String>) -> Option<String> {
        match encrypted {
            Some(data) if !data.is_empty() => match self.vault.decrypt(data) {
                Ok(key) if !key.is_empty() => Some(key),
                _ => None,
            },
            _ => None,
        }
    }

    /// List all provider configurations (keys resolved in memory).
    pub fn list(&self) -> anyhow::Result<Vec<ProviderConfig>> {
        let _guard = self
            .lock
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {}", e))?;
        let conn = self.connect()?;

        let mut stmt = conn.prepare(
            "SELECT id, name, provider_type, endpoint, api_key_encrypted, models, is_local, enabled, created_at
             FROM providers ORDER BY created_at ASC",
        )?;

        let rows = stmt.query_map([], |row| {
            let models_json: String = row.get(5)?;
            let models: Vec<String> = serde_json::from_str(&models_json).unwrap_or_default();
            let encrypted_key: Option<String> = row.get(4)?;
            let has_encrypted_key = encrypted_key.as_deref().is_some_and(|s| !s.is_empty());

            Ok((
                ProviderConfig {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    provider_type: ProviderType::from_str(&row.get::<_, String>(2)?),
                    endpoint: row.get(3)?,
                    api_key: None,
                    models,
                    is_local: row.get::<_, i32>(6)? != 0,
                    enabled: row.get::<_, i32>(7)? != 0,
                    created_at: row.get(8)?,
                    has_encrypted_key,
                },
                encrypted_key,
            ))
        })?;

        let mut providers = Vec::new();
        for row in rows {
            let (mut config, encrypted_key) = row?;
            config.api_key = self.resolve_key(&config.id, &encrypted_key);
            providers.push(config);
        }
        Ok(providers)
    }

    /// Resolve a provider's key: decrypt stored ciphertext if present,
    /// otherwise fall back to the `SWARM_PROVIDER_KEY_<ID>` env var.
    fn resolve_key(&self, id: &str, encrypted_key: &Option<String>) -> Option<String> {
        if let Some(key) = self.decrypt_key(encrypted_key) {
            return Some(key);
        }
        // Env-var fallback (used when no keychain is available, so nothing
        // was persisted). Only consult it when there is no stored ciphertext
        // — a present-but-undecryptable ciphertext is `Stranded`, not a
        // silent env fallback.
        let has_ciphertext = encrypted_key.as_deref().is_some_and(|s| !s.is_empty());
        if has_ciphertext {
            return None;
        }
        std::env::var(env_key_for(id))
            .ok()
            .filter(|v| !v.is_empty())
    }

    /// Get a provider by id.
    pub fn get(&self, id: &str) -> anyhow::Result<Option<ProviderConfig>> {
        let providers = self.list()?;
        Ok(providers.into_iter().find(|p| p.id == id))
    }

    /// Add a new provider (key encrypted before storage when possible).
    pub fn add(&self, config: ProviderConfig) -> anyhow::Result<String> {
        let encrypted_key = self.encrypt_key(&config.id, &config.api_key)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {}", e))?;
        let conn = self.connect()?;
        let models_json = serde_json::to_string(&config.models)?;

        conn.execute(
            "INSERT INTO providers (id, name, provider_type, endpoint, api_key_encrypted, models, is_local, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                config.id,
                config.name,
                config.provider_type.as_str(),
                config.endpoint,
                encrypted_key,
                models_json,
                config.is_local as i32,
                config.enabled as i32,
                config.created_at,
            ],
        )?;

        Ok(config.id)
    }

    /// Upsert a provider: insert if new, otherwise replace.
    pub fn upsert(&self, config: ProviderConfig) -> anyhow::Result<String> {
        if self.get(&config.id)?.is_some() {
            self.update(config.clone())?;
            Ok(config.id)
        } else {
            self.add(config)
        }
    }

    /// Update an existing provider (key re-encrypted before storage).
    pub fn update(&self, config: ProviderConfig) -> anyhow::Result<()> {
        let encrypted_key = self.encrypt_key(&config.id, &config.api_key)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {}", e))?;
        let conn = self.connect()?;
        let models_json = serde_json::to_string(&config.models)?;

        conn.execute(
            "UPDATE providers SET name=?2, provider_type=?3, endpoint=?4, api_key_encrypted=?5,
             models=?6, is_local=?7, enabled=?8 WHERE id=?1",
            params![
                config.id,
                config.name,
                config.provider_type.as_str(),
                config.endpoint,
                encrypted_key,
                models_json,
                config.is_local as i32,
                config.enabled as i32,
            ],
        )?;

        Ok(())
    }

    /// Delete a provider by id.
    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        let _guard = self
            .lock
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {}", e))?;
        let conn = self.connect()?;
        conn.execute("DELETE FROM providers WHERE id=?1", params![id])?;
        Ok(())
    }

    /// Update the models list for a provider.
    pub fn set_models(&self, id: &str, models: Vec<String>) -> anyhow::Result<()> {
        let _guard = self
            .lock
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {}", e))?;
        let conn = self.connect()?;
        let models_json = serde_json::to_string(&models)?;
        conn.execute(
            "UPDATE providers SET models=?2 WHERE id=?1",
            params![id, models_json],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::crypto::{env_key_for, KeychainVault};
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;

    // Serializes env-var-mutating tests so parallel runs don't race.
    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    fn test_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = i as u8;
        }
        key
    }

    #[test]
    fn registry_crud_roundtrip() {
        let dir = tempdir().unwrap();
        let vault = KeychainVault::with_key(test_key());
        let registry = ProviderRegistry::open_with_vault(&dir.path().to_path_buf(), vault).unwrap();

        let config = ProviderConfig::new(
            "Test Local".to_string(),
            ProviderType::Ollama,
            Some("http://localhost:11434".to_string()),
            None,
        );
        let id = registry.add(config).unwrap();

        let providers = registry.list().unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "Test Local");

        let p = registry.get(&id).unwrap().unwrap();
        assert_eq!(p.id, id);

        registry.delete(&id).unwrap();
        assert!(registry.list().unwrap().is_empty());
    }

    #[test]
    fn upsert_inserts_then_updates() {
        let dir = tempdir().unwrap();
        let vault = KeychainVault::with_key(test_key());
        let registry = ProviderRegistry::open_with_vault(&dir.path().to_path_buf(), vault).unwrap();

        let mut config = ProviderConfig::new(
            "First".to_string(),
            ProviderType::OpenAI,
            None,
            Some("sk-one".to_string()),
        );
        let id = registry.upsert(config.clone()).unwrap();
        assert_eq!(registry.list().unwrap().len(), 1);

        config.id = id.clone();
        config.name = "Renamed".to_string();
        registry.upsert(config).unwrap();

        let all = registry.list().unwrap();
        assert_eq!(all.len(), 1, "upsert of same id must not duplicate");
        assert_eq!(all[0].name, "Renamed");
    }

    #[test]
    fn api_key_encrypted_at_rest() {
        let dir = tempdir().unwrap();
        let vault = KeychainVault::with_key(test_key());
        let registry = ProviderRegistry::open_with_vault(&dir.path().to_path_buf(), vault).unwrap();

        let config = ProviderConfig::new(
            "Test Cloud".to_string(),
            ProviderType::OpenAI,
            None,
            Some("sk-test-secret-key-12345".to_string()),
        );
        let id = registry.add(config).unwrap();

        let conn = Connection::open(dir.path().join("providers.db")).unwrap();
        let raw: String = conn
            .query_row(
                "SELECT api_key_encrypted FROM providers WHERE id=?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();

        assert_ne!(raw, "sk-test-secret-key-12345");
        assert!(KeychainVault::is_value_encrypted(&raw));

        let p = registry.get(&id).unwrap().unwrap();
        assert_eq!(p.api_key, Some("sk-test-secret-key-12345".to_string()));
        assert_eq!(p.key_status(), KeyStatus::Healthy);
    }

    #[test]
    fn key_status_absent_without_key() {
        let dir = tempdir().unwrap();
        let vault = KeychainVault::with_key(test_key());
        let registry = ProviderRegistry::open_with_vault(&dir.path().to_path_buf(), vault).unwrap();

        let config = ProviderConfig::new(
            "No Key".to_string(),
            ProviderType::Ollama,
            Some("http://localhost:11434".to_string()),
            None,
        );
        registry.add(config).unwrap();

        let providers = registry.list().unwrap();
        assert!(!providers[0].has_encrypted_key);
        assert_eq!(providers[0].api_key, None);
        assert_eq!(providers[0].key_status(), KeyStatus::Absent);
    }

    #[test]
    fn key_status_stranded_when_decryption_fails() {
        let dir = tempdir().unwrap();

        {
            let vault = KeychainVault::with_key(test_key());
            let registry =
                ProviderRegistry::open_with_vault(&dir.path().to_path_buf(), vault).unwrap();
            let config = ProviderConfig::new(
                "Stranded".to_string(),
                ProviderType::OpenAI,
                None,
                Some("sk-stranded-key".to_string()),
            );
            registry.add(config).unwrap();
        }

        let other_key = {
            let mut k = [0u8; 32];
            for (i, byte) in k.iter_mut().enumerate() {
                *byte = (i as u8).wrapping_add(0x55);
            }
            k
        };
        let vault = KeychainVault::with_key(other_key);
        let registry = ProviderRegistry::open_with_vault(&dir.path().to_path_buf(), vault).unwrap();
        let providers = registry.list().unwrap();
        assert_eq!(providers[0].api_key, None);
        assert!(providers[0].has_encrypted_key);
        assert_eq!(providers[0].key_status(), KeyStatus::Stranded);
    }

    #[test]
    fn no_keychain_does_not_persist_plaintext() {
        // Without a master key, a key must NOT be written to disk in any
        // form; the row stores NULL ciphertext.
        let dir = tempdir().unwrap();
        let vault = KeychainVault::without_keychain();
        let registry = ProviderRegistry::open_with_vault(&dir.path().to_path_buf(), vault).unwrap();

        let config = ProviderConfig::new(
            "Envless".to_string(),
            ProviderType::OpenAI,
            None,
            Some("sk-should-not-persist".to_string()),
        );
        let id = registry.add(config).unwrap();

        let conn = Connection::open(dir.path().join("providers.db")).unwrap();
        let raw: Option<String> = conn
            .query_row(
                "SELECT api_key_encrypted FROM providers WHERE id=?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            raw.as_deref().is_none_or(|s| s.is_empty()),
            "no keychain: plaintext key must never be written to disk"
        );
    }

    #[test]
    fn env_var_fallback_resolves_key() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempdir().unwrap();
        let vault = KeychainVault::without_keychain();
        let registry = ProviderRegistry::open_with_vault(&dir.path().to_path_buf(), vault).unwrap();

        // Stored with no key (keychain absent), then supplied via env.
        let config =
            ProviderConfig::new("Env Provider".to_string(), ProviderType::OpenAI, None, None);
        let id = registry.add(config).unwrap();

        let var = env_key_for(&id);
        std::env::set_var(&var, "sk-from-env");

        let p = registry.get(&id).unwrap().unwrap();
        assert_eq!(p.api_key, Some("sk-from-env".to_string()));
        assert_eq!(p.key_status(), KeyStatus::Healthy);

        std::env::remove_var(&var);

        // Once the env var is gone the key is absent again.
        let p2 = registry.get(&id).unwrap().unwrap();
        assert_eq!(p2.api_key, None);
        assert_eq!(p2.key_status(), KeyStatus::Absent);
    }

    #[test]
    fn stranded_ciphertext_does_not_fall_back_to_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempdir().unwrap();

        let id = {
            let vault = KeychainVault::with_key(test_key());
            let registry =
                ProviderRegistry::open_with_vault(&dir.path().to_path_buf(), vault).unwrap();
            let config = ProviderConfig::new(
                "Stranded".to_string(),
                ProviderType::OpenAI,
                None,
                Some("sk-real".to_string()),
            );
            registry.add(config).unwrap()
        };

        // Set an env var that would resolve if env fallback were consulted.
        let var = env_key_for(&id);
        std::env::set_var(&var, "sk-impostor");

        let other_key = {
            let mut k = [0u8; 32];
            for (i, byte) in k.iter_mut().enumerate() {
                *byte = (i as u8).wrapping_add(0x11);
            }
            k
        };
        let vault = KeychainVault::with_key(other_key);
        let registry = ProviderRegistry::open_with_vault(&dir.path().to_path_buf(), vault).unwrap();
        let p = registry.get(&id).unwrap().unwrap();

        std::env::remove_var(&var);

        // Ciphertext present but undecryptable: Stranded, NOT the env value.
        assert_eq!(p.api_key, None);
        assert_eq!(p.key_status(), KeyStatus::Stranded);
    }
}
