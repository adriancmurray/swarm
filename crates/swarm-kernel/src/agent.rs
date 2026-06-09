#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    /// Built-in selection. INVARIANT: when `custom` is `Some`, this field is a
    /// placeholder (`AgentChoice::Auto`) and MUST NOT drive dispatch or
    /// availability — consumers check `custom` first.
    pub agent: AgentChoice,
    pub model: Option<String>,
    /// Config-defined backend id (a `[backend.<id>]` descriptor), resolved via
    /// the `BackendRegistry` at dispatch. `None` for built-ins.
    pub custom: Option<String>,
}

impl AgentSpec {
    /// A spec selecting a built-in backend.
    pub fn builtin(agent: AgentChoice, model: Option<String>) -> Self {
        Self {
            agent,
            model,
            custom: None,
        }
    }

    /// A spec selecting a config-defined backend by id. `agent` holds the
    /// documented placeholder and is never consulted while `custom` is `Some`.
    pub fn for_custom(id: impl Into<String>, model: Option<String>) -> Self {
        Self {
            agent: AgentChoice::Auto,
            model,
            custom: Some(id.into()),
        }
    }

    /// The dispatch id for this spec: the custom backend id when present,
    /// else the built-in's canonical name.
    pub fn backend_id(&self) -> &str {
        self.custom
            .as_deref()
            .unwrap_or_else(|| agent_name(self.agent))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentChoice {
    Codex,
    Claude,
    Auto,
}

impl AgentChoice {
    pub fn display_name(self) -> &'static str {
        match self {
            AgentChoice::Codex => "Codex",
            AgentChoice::Claude => "Claude",
            AgentChoice::Auto => "auto-selected agent",
        }
    }

    pub fn command_name(self) -> &'static str {
        match self {
            AgentChoice::Codex => "codex",
            AgentChoice::Claude => "claude",
            AgentChoice::Auto => "agent",
        }
    }
}

pub fn agent_name(agent: AgentChoice) -> &'static str {
    match agent {
        AgentChoice::Codex => "codex",
        AgentChoice::Claude => "claude",
        AgentChoice::Auto => "auto",
    }
}

pub fn describe_spec(spec: &AgentSpec) -> String {
    let id = spec.backend_id();
    match spec.model.as_deref() {
        Some(model) => format!("{id}:{model}"),
        None => id.to_string(),
    }
}
