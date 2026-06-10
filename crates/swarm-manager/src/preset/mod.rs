//! Agent presets and runtime configuration.
//!
//! A [`Preset`] is a saved, named agent configuration: which provider and
//! model to use, the system prompt, sampling parameters, and an optional
//! reference to a stored provider credential. [`PresetStore`] is a
//! JSON-backed collection of presets with an active-preset pointer, loaded
//! from and saved to a data directory.
//!
//! API keys are never persisted here; [`AgentConfig`] carries the key only
//! in memory at run time.

use crate::provider::ProviderType;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Visibility lever — which callers may dispatch this preset.
///
/// These are plain marker variants in v1: they describe intent but are not
/// enforced by the manager core. A later arc can attach enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConsumerPolicy {
    /// Any caller may dispatch.
    Public,
    /// Only explicitly allowed callers may dispatch.
    #[default]
    Allowlist,
    /// Internal use only; not exposed to external callers.
    Internal,
}

/// Ownership model — a marker for where the agent is considered to live.
///
/// Plain marker variants in v1; no sync behaviour is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentOwner {
    /// Personal agent, conceptually portable across a user's devices.
    User,
    /// Workstation agent, tied to this node only.
    #[default]
    Node,
}

fn default_temperature() -> f32 {
    0.7
}

fn default_max_tool_iterations() -> usize {
    20
}

fn default_system_prompt() -> String {
    "You are a helpful AI assistant.".to_string()
}

/// Runtime agent configuration derived from a [`Preset`].
///
/// This is the shape the single-agent loop consumes. The API key is held
/// in memory only and is never serialized to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Provider type to drive.
    pub provider: ProviderType,
    /// Model identifier (e.g. `"llama3.2"`, `"gpt-5.5"`).
    pub model: String,
    /// Custom endpoint override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// System prompt.
    pub system_prompt: String,
    /// Maximum tool iterations per request.
    pub max_tool_iterations: usize,
    /// Sampling temperature.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Maximum tokens to generate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    /// API key — held in memory only, never persisted.
    #[serde(skip)]
    pub api_key: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            provider: ProviderType::Ollama,
            model: "llama3.2".to_string(),
            endpoint: None,
            system_prompt: default_system_prompt(),
            max_tool_iterations: default_max_tool_iterations(),
            temperature: default_temperature(),
            max_tokens: None,
            api_key: None,
        }
    }
}

impl AgentConfig {
    /// Resolve the effective endpoint: the explicit override, otherwise the
    /// provider's default.
    pub fn effective_endpoint(&self) -> Option<String> {
        self.endpoint
            .clone()
            .or_else(|| self.provider.default_endpoint().map(|s| s.to_string()))
    }

    /// Whether the provider is configured (a key, if required, is checked at
    /// run time, not here).
    pub fn is_configured(&self) -> bool {
        !matches!(self.provider, ProviderType::None)
    }
}

/// A partial update to an [`AgentConfig`]. Absent fields are left unchanged.
#[derive(Debug, Default, Deserialize)]
pub struct ConfigUpdate {
    #[serde(default)]
    pub provider: Option<ProviderType>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub max_tool_iterations: Option<usize>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub api_key: Option<String>,
}

impl AgentConfig {
    /// Apply a partial update in place.
    pub fn apply_update(&mut self, update: ConfigUpdate) {
        if let Some(provider) = update.provider {
            self.provider = provider;
        }
        if let Some(model) = update.model {
            self.model = model;
        }
        if update.endpoint.is_some() {
            self.endpoint = update.endpoint;
        }
        if let Some(prompt) = update.system_prompt {
            self.system_prompt = prompt;
        }
        if let Some(max) = update.max_tool_iterations {
            self.max_tool_iterations = max;
        }
        if let Some(temp) = update.temperature {
            self.temperature = temp;
        }
        if update.max_tokens.is_some() {
            self.max_tokens = update.max_tokens;
        }
        if update.api_key.is_some() {
            self.api_key = update.api_key;
        }
    }
}

/// A saved, named agent configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    /// Unique identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Ownership marker.
    #[serde(default)]
    pub owner: AgentOwner,
    /// Reference to a stored provider credential. When set, the runtime
    /// resolves the provider from the registry; the inline
    /// provider/model/endpoint fields act as a fallback otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_config_id: Option<String>,
    /// Provider type (used when `provider_config_id` is unset).
    pub provider: ProviderType,
    /// Model identifier.
    pub model: String,
    /// Custom endpoint override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// System prompt.
    pub system_prompt: String,
    /// Skill names this preset would like injected.
    ///
    /// The live skill-injection path is the native backend descriptor's
    /// `skills` field, which is resolved through [`crate::skills::SkillSet`]
    /// (prompt composition + tool gating). Presets are not yet wired into
    /// native dispatch, so this field is persisted for forward compatibility
    /// but is NOT honoured at dispatch time — do not assume setting it gates
    /// tools or injects guidance. When preset-driven native dispatch lands, it
    /// will route these names through the same `SkillSet`.
    #[serde(default)]
    pub enabled_skills: Vec<String>,
    /// Opaque permission override payload (highest-priority rule layer).
    /// Carried through verbatim; interpreted by the permission layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_overrides: Option<serde_json::Value>,
    /// Sampling temperature.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Maximum tokens to generate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    /// Maximum tool iterations per request.
    #[serde(default = "default_max_tool_iterations")]
    pub max_tool_iterations: usize,
    /// Whether this is the default preset.
    #[serde(default)]
    pub is_default: bool,
    /// Visibility lever.
    #[serde(default)]
    pub consumer_policy: ConsumerPolicy,
}

impl Preset {
    /// Create a new preset with a freshly generated id.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        provider: ProviderType,
        model: String,
        endpoint: Option<String>,
        system_prompt: String,
        owner: Option<AgentOwner>,
        temperature: f32,
        max_tokens: Option<usize>,
        provider_config_id: Option<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            owner: owner.unwrap_or_default(),
            provider_config_id,
            provider,
            model,
            endpoint,
            system_prompt,
            enabled_skills: Vec::new(),
            permission_overrides: None,
            temperature,
            max_tokens,
            max_tool_iterations: default_max_tool_iterations(),
            is_default: false,
            consumer_policy: ConsumerPolicy::default(),
        }
    }

    /// Convert this preset into the runtime [`AgentConfig`] the agent loop
    /// consumes. The API key is supplied separately at run time.
    pub fn to_config(&self) -> AgentConfig {
        AgentConfig {
            provider: self.provider,
            model: self.model.clone(),
            endpoint: self.endpoint.clone(),
            system_prompt: self.system_prompt.clone(),
            max_tool_iterations: self.max_tool_iterations,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            api_key: None,
        }
    }
}

/// Request to create or update a [`Preset`].
#[derive(Debug, Default, Deserialize)]
pub struct PresetRequest {
    pub name: String,
    pub provider: ProviderType,
    pub model: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    pub system_prompt: String,
    #[serde(default)]
    pub enabled_skills: Option<Vec<String>>,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub owner: Option<AgentOwner>,
    #[serde(default)]
    pub permission_overrides: Option<serde_json::Value>,
    /// Reference to a stored provider credential.
    #[serde(default)]
    pub provider_config_id: Option<String>,
    /// Visibility lever; defaults to `Allowlist` when unspecified.
    #[serde(default)]
    pub consumer_policy: Option<ConsumerPolicy>,
    /// Per-preset cap; defaults to 20 when unspecified.
    #[serde(default)]
    pub max_tool_iterations: Option<usize>,
}

/// JSON-backed collection of presets with an active-preset pointer.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PresetStore {
    pub presets: Vec<Preset>,
    /// Id of the active preset, if any.
    pub active_preset_id: Option<String>,
}

const PRESETS_FILE: &str = "presets.json";

impl PresetStore {
    /// Load presets from `data_dir/presets.json`, or return an empty store
    /// when the file is absent or unreadable.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join(PRESETS_FILE);
        if path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                if let Ok(store) = serde_json::from_str(&contents) {
                    return store;
                }
            }
        }
        Self::default()
    }

    /// Save presets to `data_dir/presets.json`, creating the directory if
    /// needed.
    pub fn save(&self, data_dir: &Path) -> Result<(), std::io::Error> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join(PRESETS_FILE);
        let contents = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, contents)
    }

    /// Add a new preset built from a request and return a clone of it.
    pub fn add(&mut self, request: PresetRequest) -> Preset {
        let mut preset = Preset::new(
            request.name,
            request.provider,
            request.model,
            request.endpoint,
            request.system_prompt,
            request.owner,
            request.temperature,
            request.max_tokens,
            request.provider_config_id,
        );
        preset.enabled_skills = request.enabled_skills.unwrap_or_default();
        if let Some(overrides) = request.permission_overrides {
            preset.permission_overrides = Some(overrides);
        }
        if let Some(p) = request.consumer_policy {
            preset.consumer_policy = p;
        }
        if let Some(n) = request.max_tool_iterations {
            preset.max_tool_iterations = n;
        }
        self.presets.push(preset.clone());
        preset
    }

    /// Update an existing preset in place, returning a clone on success.
    pub fn update(&mut self, id: &str, request: PresetRequest) -> Option<Preset> {
        if let Some(preset) = self.presets.iter_mut().find(|p| p.id == id) {
            preset.name = request.name;
            preset.provider = request.provider;
            preset.model = request.model;
            preset.endpoint = request.endpoint;
            preset.system_prompt = request.system_prompt;
            if let Some(skills) = request.enabled_skills {
                preset.enabled_skills = skills;
            }
            preset.provider_config_id = request.provider_config_id;
            if let Some(overrides) = request.permission_overrides {
                preset.permission_overrides = Some(overrides);
            }
            preset.temperature = request.temperature;
            preset.max_tokens = request.max_tokens;
            if let Some(p) = request.consumer_policy {
                preset.consumer_policy = p;
            }
            if let Some(n) = request.max_tool_iterations {
                preset.max_tool_iterations = n;
            }
            return Some(preset.clone());
        }
        None
    }

    /// Delete a preset by id. Clears the active pointer if it matched.
    pub fn delete(&mut self, id: &str) -> bool {
        let len_before = self.presets.len();
        self.presets.retain(|p| p.id != id);
        if self.active_preset_id.as_deref() == Some(id) {
            self.active_preset_id = None;
        }
        self.presets.len() < len_before
    }

    /// Delete a preset, rejecting deletion of the default preset.
    pub fn delete_safe(&mut self, id: &str) -> Result<bool, String> {
        if let Some(p) = self.presets.iter().find(|p| p.id == id) {
            if p.is_default {
                return Err(format!("cannot delete default preset '{id}'"));
            }
        }
        Ok(self.delete(id))
    }

    /// Get a preset by id.
    pub fn get(&self, id: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// Set the active preset id.
    pub fn set_active(&mut self, id: Option<String>) {
        self.active_preset_id = id;
    }

    /// Get the active preset, if the active id resolves to one.
    pub fn get_active(&self) -> Option<&Preset> {
        self.active_preset_id.as_ref().and_then(|id| self.get(id))
    }

    /// List all presets.
    pub fn list(&self) -> &[Preset] {
        &self.presets
    }

    /// Id of the preset flagged as default, if any.
    pub fn default_id(&self) -> Option<String> {
        self.presets
            .iter()
            .find(|p| p.is_default)
            .map(|p| p.id.clone())
    }

    /// Ensure exactly one preset is flagged default. If several are, keep the
    /// first and clear the rest; if none are and the store is non-empty, mark
    /// the head. The empty case is left to the caller.
    pub fn ensure_single_default(&mut self) {
        let defaults: Vec<usize> = self
            .presets
            .iter()
            .enumerate()
            .filter_map(|(i, p)| if p.is_default { Some(i) } else { None })
            .collect();
        if defaults.is_empty() {
            if let Some(first) = self.presets.first_mut() {
                first.is_default = true;
            }
            return;
        }
        for &i in defaults.iter().skip(1) {
            self.presets[i].is_default = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request(name: &str) -> PresetRequest {
        PresetRequest {
            name: name.to_string(),
            provider: ProviderType::Ollama,
            model: "llama3.2".to_string(),
            endpoint: None,
            system_prompt: "Test prompt".to_string(),
            enabled_skills: None,
            temperature: 0.7,
            max_tokens: None,
            owner: None,
            permission_overrides: None,
            provider_config_id: None,
            consumer_policy: None,
            max_tool_iterations: None,
        }
    }

    #[test]
    fn preset_serialize_round_trip() {
        let mut preset = Preset::new(
            "Reviewer".into(),
            ProviderType::Anthropic,
            "claude-sonnet-4-6".into(),
            Some("https://example.test/v1".into()),
            "Review carefully".into(),
            Some(AgentOwner::User),
            0.3,
            Some(2048),
            Some("cfg-1".into()),
        );
        preset.enabled_skills = vec!["code-review".into()];
        preset.consumer_policy = ConsumerPolicy::Public;
        preset.max_tool_iterations = 12;

        let json = serde_json::to_string(&preset).unwrap();
        let back: Preset = serde_json::from_str(&json).unwrap();

        assert_eq!(back.id, preset.id);
        assert_eq!(back.name, preset.name);
        assert_eq!(back.owner, AgentOwner::User);
        assert_eq!(back.provider, ProviderType::Anthropic);
        assert_eq!(back.model, preset.model);
        assert_eq!(back.endpoint.as_deref(), Some("https://example.test/v1"));
        assert_eq!(back.enabled_skills, vec!["code-review".to_string()]);
        assert_eq!(back.consumer_policy, ConsumerPolicy::Public);
        assert_eq!(back.provider_config_id.as_deref(), Some("cfg-1"));
        assert_eq!(back.max_tool_iterations, 12);
        assert_eq!(back.max_tokens, Some(2048));
    }

    #[test]
    fn consumer_policy_round_trips_as_string() {
        for p in [
            ConsumerPolicy::Public,
            ConsumerPolicy::Allowlist,
            ConsumerPolicy::Internal,
        ] {
            let s = serde_json::to_string(&p).unwrap();
            let back: ConsumerPolicy = serde_json::from_str(&s).unwrap();
            assert_eq!(p, back);
        }
        assert_eq!(
            serde_json::to_string(&ConsumerPolicy::Public).unwrap(),
            "\"public\""
        );
    }

    #[test]
    fn agent_owner_defaults_to_node() {
        assert_eq!(AgentOwner::default(), AgentOwner::Node);
        let s = serde_json::to_string(&AgentOwner::User).unwrap();
        assert_eq!(s, "\"user\"");
    }

    #[test]
    fn consumer_policy_defaults_to_allowlist() {
        assert_eq!(ConsumerPolicy::default(), ConsumerPolicy::Allowlist);
    }

    #[test]
    fn new_preset_has_safe_defaults() {
        let preset = Preset::new(
            "T".into(),
            ProviderType::Ollama,
            "m".into(),
            None,
            "p".into(),
            None,
            0.7,
            None,
            None,
        );
        assert_eq!(preset.max_tool_iterations, 20);
        assert!(!preset.is_default);
        assert_eq!(preset.consumer_policy, ConsumerPolicy::Allowlist);
        assert_eq!(preset.owner, AgentOwner::Node);
        assert!(preset.enabled_skills.is_empty());
        assert!(preset.permission_overrides.is_none());
    }

    #[test]
    fn store_add_get_update_delete() {
        let mut store = PresetStore::default();
        let preset = store.add(sample_request("First"));
        assert_eq!(store.list().len(), 1);
        assert!(store.get(&preset.id).is_some());

        let updated = store
            .update(
                &preset.id,
                PresetRequest {
                    model: "llama3.3".into(),
                    system_prompt: "Updated".into(),
                    ..sample_request("Renamed")
                },
            )
            .unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.model, "llama3.3");
        assert_eq!(store.get(&preset.id).unwrap().system_prompt, "Updated");

        assert!(store.update("missing", sample_request("X")).is_none());

        assert!(store.delete(&preset.id));
        assert!(!store.delete(&preset.id));
        assert_eq!(store.list().len(), 0);
    }

    #[test]
    fn store_round_trips_enabled_skills() {
        let mut store = PresetStore::default();
        let preset = store.add(PresetRequest {
            enabled_skills: Some(vec!["code-review".into()]),
            ..sample_request("Reviewer")
        });
        assert_eq!(preset.enabled_skills, vec!["code-review".to_string()]);
        assert_eq!(
            store.get(&preset.id).unwrap().enabled_skills,
            vec!["code-review".to_string()]
        );
    }

    #[test]
    fn set_active_and_clear_on_delete() {
        let mut store = PresetStore::default();
        let preset = store.add(sample_request("Active"));
        assert!(store.get_active().is_none());

        store.set_active(Some(preset.id.clone()));
        assert_eq!(
            store.get_active().map(|p| p.id.clone()),
            Some(preset.id.clone())
        );

        store.delete(&preset.id);
        assert!(store.get_active().is_none());
        assert!(store.active_preset_id.is_none());
    }

    #[test]
    fn default_id_and_single_default_resolution() {
        let mut store = PresetStore::default();
        let mut a = Preset::new(
            "A".into(),
            ProviderType::Ollama,
            "m".into(),
            None,
            "p".into(),
            None,
            0.7,
            None,
            None,
        );
        a.is_default = true;
        let aid = a.id.clone();
        let mut b = Preset::new(
            "B".into(),
            ProviderType::Ollama,
            "m".into(),
            None,
            "p".into(),
            None,
            0.7,
            None,
            None,
        );
        b.is_default = true;
        store.presets.push(a);
        store.presets.push(b);

        store.ensure_single_default();
        assert_eq!(store.presets.iter().filter(|p| p.is_default).count(), 1);
        assert_eq!(store.default_id().as_deref(), Some(aid.as_str()));
    }

    #[test]
    fn ensure_single_default_marks_head_when_none() {
        let mut store = PresetStore::default();
        store.add(sample_request("Only"));
        assert!(store.default_id().is_none());
        store.ensure_single_default();
        assert_eq!(
            store.default_id().as_deref(),
            Some(store.presets[0].id.as_str())
        );
    }

    #[test]
    fn delete_safe_rejects_default() {
        let mut store = PresetStore::default();
        let mut d = Preset::new(
            "Default".into(),
            ProviderType::Ollama,
            "m".into(),
            None,
            "p".into(),
            None,
            0.7,
            None,
            None,
        );
        d.is_default = true;
        let did = d.id.clone();
        store.presets.push(d);
        let other = store.add(sample_request("Other"));

        assert!(store.delete_safe(&did).is_err());
        assert!(store.delete_safe(&other.id).is_ok());
    }

    #[test]
    fn store_load_save_round_trip_in_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = PresetStore::default();
        let preset = store.add(sample_request("Persisted"));
        store.set_active(Some(preset.id.clone()));
        store.save(dir.path()).unwrap();

        let loaded = PresetStore::load(dir.path());
        assert_eq!(loaded.list().len(), 1);
        assert_eq!(loaded.get(&preset.id).unwrap().name, "Persisted");
        assert_eq!(loaded.active_preset_id, Some(preset.id));
    }

    #[test]
    fn load_missing_dir_returns_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        let store = PresetStore::load(&missing);
        assert!(store.list().is_empty());
        assert!(store.active_preset_id.is_none());
    }

    #[test]
    fn to_config_maps_preset_fields() {
        let mut preset = Preset::new(
            "Cfg".into(),
            ProviderType::OpenAI,
            "gpt-5.5".into(),
            Some("https://api.example.test/v1".into()),
            "You are helpful.".into(),
            None,
            0.5,
            Some(512),
            None,
        );
        preset.max_tool_iterations = 7;

        let config = preset.to_config();
        assert_eq!(config.provider, ProviderType::OpenAI);
        assert_eq!(config.model, "gpt-5.5");
        assert_eq!(
            config.endpoint.as_deref(),
            Some("https://api.example.test/v1")
        );
        assert_eq!(config.system_prompt, "You are helpful.");
        assert_eq!(config.max_tool_iterations, 7);
        assert_eq!(config.temperature, 0.5);
        assert_eq!(config.max_tokens, Some(512));
        assert!(
            config.api_key.is_none(),
            "api key is never carried from a preset"
        );
    }

    #[test]
    fn agent_config_effective_endpoint_falls_back_to_provider_default() {
        let mut config = AgentConfig {
            provider: ProviderType::OpenAI,
            endpoint: None,
            ..AgentConfig::default()
        };
        assert_eq!(
            config.effective_endpoint().as_deref(),
            Some("https://api.openai.com/v1")
        );
        config.endpoint = Some("http://localhost:9000".into());
        assert_eq!(
            config.effective_endpoint().as_deref(),
            Some("http://localhost:9000")
        );
    }

    #[test]
    fn agent_config_apply_update_is_partial() {
        let mut config = AgentConfig::default();
        let original_prompt = config.system_prompt.clone();
        config.apply_update(ConfigUpdate {
            model: Some("custom-model".into()),
            temperature: Some(0.1),
            ..ConfigUpdate::default()
        });
        assert_eq!(config.model, "custom-model");
        assert_eq!(config.temperature, 0.1);
        assert_eq!(config.system_prompt, original_prompt);
    }

    #[test]
    fn agent_config_is_configured_false_for_none_provider() {
        let config = AgentConfig {
            provider: ProviderType::None,
            ..AgentConfig::default()
        };
        assert!(!config.is_configured());
        assert!(AgentConfig::default().is_configured());
    }
}
