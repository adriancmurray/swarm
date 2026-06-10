use std::env;
use std::path::{Path, PathBuf};

use crate::agent::{AgentChoice, AgentSpec};
use crate::profiles;
use crate::prompts::{build_audit_prompt, build_design_prompt};
use crate::resolver::{agent_available, home_dir, running_inside_codex};
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub prompt: String,
    pub cwd: PathBuf,
    pub timeout_secs: u64,
    pub quiet: bool,
    pub agent: AgentChoice,
    /// Config-defined backend id when `--agent <id>` named a `[backend.<id>]`
    /// descriptor rather than a built-in. Takes precedence over `agent` at
    /// dispatch; resolved (and validated) against the `BackendRegistry`.
    pub agent_custom: Option<String>,
    pub model: Option<String>,
    pub persona: Option<String>,
    pub background: bool,
    pub allow_bypass_permissions: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerSpec {
    pub role: String,
    pub spec: AgentSpec,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwarmArgs {
    pub prompt: String,
    pub cwd: PathBuf,
    pub timeout_secs: u64,
    pub manager: AgentSpec,
    pub workers: Vec<WorkerSpec>,
    pub parent: Option<String>,
    pub slice: Option<String>,
    /// Opt-in auto-context injection override. `Some(true)` / `Some(false)`
    /// from `--context` / `--no-context` CLI flags override the config value.
    /// `None` means "use the config key" (which defaults to false).
    pub inject_context: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscussArgs {
    pub prompt: String,
    pub cwd: PathBuf,
    pub timeout_secs: u64,
    pub manager: AgentSpec,
    pub participants: Vec<WorkerSpec>,
    pub rounds: u32,
    pub parent: Option<String>,
    pub slice: Option<String>,
    /// Opt-in API-docs follow-up worker override. `Some(true)` / `Some(false)`
    /// from `--docs`/`--api-docs` / `--no-docs` CLI flags override the config
    /// value. `None` means "use `config.settings.docs_default`" (which defaults
    /// to false). `parse_audit_args` sets `Some(true)` to preserve audit's
    /// built-in docs-ON behaviour.
    pub docs: Option<bool>,
    pub docs_agent: AgentSpec,
    pub profile_helpers: bool,
}

pub fn parse_args<I>(raw: I) -> Result<Args, String>
where
    I: IntoIterator<Item = String>,
{
    let raw: Vec<String> = raw.into_iter().collect();
    if raw
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        print_help(DEFAULT_TIMEOUT_SECS);
        std::process::exit(0);
    }

    let config = crate::config::load_config();
    let mut prompt = None;
    let mut cwd = env::current_dir().map_err(|err| format!("Error reading cwd: {err}"))?;
    let mut timeout_secs = load_default_timeout();
    let mut agent = load_default_agent();
    let mut agent_custom: Option<String> = None;
    let mut agent_explicit = false;
    let mut model = None;
    let mut persona = config.settings.direct_persona.clone();
    let mut quiet = false;
    let mut background = false;
    let mut allow_bypass_permissions = false;

    let mut iter = raw.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help(timeout_secs);
                std::process::exit(0);
            }
            "--" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: -- requires a prompt argument".to_string())?;
                if prompt.is_some() {
                    return Err(format!("Error: unexpected extra argument `{value}`"));
                }
                prompt = Some(value);
                if let Some(extra) = iter.next() {
                    return Err(format!("Error: unexpected extra argument `{extra}`"));
                }
            }
            "--quiet" => quiet = true,
            "--background" => background = true,
            "--allow-bypass-permissions" => allow_bypass_permissions = true,
            "--persona" | "--profile" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("Error: {arg} requires a value"))?;
                persona = Some(value);
            }
            "--no-persona" | "--no-profile" => persona = None,
            "--agent" | "--backend" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("Error: {arg} requires a value"))?;
                let spec = parse_agent_spec_struct(&value)?;
                agent = spec.agent;
                agent_custom = spec.custom;
                model = spec.model.or(model);
                agent_explicit = true;
            }
            "--model" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --model requires a value".to_string())?;
                model = Some(value);
            }
            "--cwd" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --cwd requires a value".to_string())?;
                cwd = PathBuf::from(value);
            }
            "--timeout" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --timeout requires a value".to_string())?;
                timeout_secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("Error: invalid --timeout value `{value}`"))?;
            }
            _ if arg.starts_with("--cwd=") => {
                cwd = PathBuf::from(arg.strip_prefix("--cwd=").unwrap_or_default());
            }
            _ if arg.starts_with("--timeout=") => {
                let value = arg.strip_prefix("--timeout=").unwrap_or_default();
                timeout_secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("Error: invalid --timeout value `{value}`"))?;
            }
            _ if arg.starts_with("--agent=") => {
                let value = arg.strip_prefix("--agent=").unwrap_or_default();
                let spec = parse_agent_spec_struct(value)?;
                agent = spec.agent;
                agent_custom = spec.custom;
                model = spec.model.or(model);
                agent_explicit = true;
            }
            _ if arg.starts_with("--backend=") => {
                let value = arg.strip_prefix("--backend=").unwrap_or_default();
                let spec = parse_agent_spec_struct(value)?;
                agent = spec.agent;
                agent_custom = spec.custom;
                model = spec.model.or(model);
                agent_explicit = true;
            }
            _ if arg.starts_with("--model=") => {
                model = Some(arg.strip_prefix("--model=").unwrap_or_default().to_string());
            }
            _ if arg.starts_with("--persona=") => {
                persona = Some(
                    arg.strip_prefix("--persona=")
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            _ if arg.starts_with("--profile=") => {
                persona = Some(
                    arg.strip_prefix("--profile=")
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            _ if arg.starts_with('-') => return Err(format!("Error: unknown option `{arg}`")),
            _ => {
                if prompt.is_some() {
                    return Err(format!("Error: unexpected extra argument `{arg}`"));
                }
                prompt = Some(arg);
            }
        }
    }

    let prompt = prompt.ok_or_else(|| "Error: missing prompt argument".to_string())?;
    if let Some(persona_name) = persona.as_deref() {
        apply_direct_persona_agent_default(persona_name, &mut agent, &mut model, agent_explicit)?;
    }
    Ok(Args {
        prompt,
        cwd,
        timeout_secs,
        quiet,
        agent,
        agent_custom,
        model,
        persona,
        background,
        allow_bypass_permissions,
    })
}

fn apply_direct_persona_agent_default(
    persona: &str,
    agent: &mut AgentChoice,
    model: &mut Option<String>,
    agent_explicit: bool,
) -> Result<(), String> {
    let normalized = normalize_persona(persona);
    if matches!(
        normalized.as_str(),
        "none"
            | "off"
            | "disabled"
            | "compact-manager"
            | "manager"
            | "metadirector"
            | "meta-director"
            | "gemini-large-context-manager"
            | "large-context-manager"
            | "wide-context-manager"
            | "gemini-manager"
            | "compact-worker"
            | "worker"
            | "handoff"
    ) {
        return Ok(());
    }

    let profile = profiles::profile_by_id_or_role(persona).ok_or_else(|| {
        format!(
            "Error: unknown direct persona `{persona}`. Use compact-manager, compact-worker, or an id/role from `agent-swarm profiles`."
        )
    })?;
    if !agent_explicit {
        let parsed = parse_agent_spec(profile.default_agent)?;
        *agent = parsed.0;
        *model = parsed.1.or_else(|| model.clone());
    }
    Ok(())
}

fn normalize_persona(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

pub fn parse_swarm_args<I>(raw: I) -> Result<SwarmArgs, String>
where
    I: IntoIterator<Item = String>,
{
    let config = crate::config::load_config();
    let mut prompt = None;
    let mut cwd = env::current_dir().map_err(|err| format!("Error reading cwd: {err}"))?;
    let mut timeout_secs = load_default_timeout();
    let mut manager = config
        .swarm
        .default_manager
        .as_deref()
        .and_then(|spec| parse_agent_spec_struct(spec).ok())
        .unwrap_or_else(default_manager);
    let mut workers = Vec::new();
    let mut parent = None;
    let mut slice = None;
    let mut inject_context: Option<bool> = None;

    let mut iter = raw.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help(timeout_secs);
                std::process::exit(0);
            }
            "--manager" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --manager requires a value".to_string())?;
                manager = parse_agent_spec_struct(&value)?;
            }
            "--worker" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --worker requires a value".to_string())?;
                workers.push(parse_worker_spec(&value)?);
            }
            "--cwd" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --cwd requires a value".to_string())?;
                cwd = PathBuf::from(value);
            }
            "--timeout" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --timeout requires a value".to_string())?;
                timeout_secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("Error: invalid --timeout value `{value}`"))?;
            }
            "--context" => inject_context = Some(true),
            "--no-context" => inject_context = Some(false),
            "--parent" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --parent requires a value".to_string())?;
                parent = Some(value);
            }
            "--slice" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --slice requires a value".to_string())?;
                slice = Some(value);
            }
            _ if arg.starts_with("--manager=") => {
                manager = parse_agent_spec_struct(arg.strip_prefix("--manager=").unwrap_or(""))?;
            }
            _ if arg.starts_with("--worker=") => {
                workers.push(parse_worker_spec(
                    arg.strip_prefix("--worker=").unwrap_or(""),
                )?);
            }
            _ if arg.starts_with("--parent=") => {
                parent = Some(
                    arg.strip_prefix("--parent=")
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            _ if arg.starts_with("--slice=") => {
                slice = Some(arg.strip_prefix("--slice=").unwrap_or_default().to_string());
            }
            _ if arg.starts_with("--cwd=") => {
                cwd = PathBuf::from(arg.strip_prefix("--cwd=").unwrap_or_default());
            }
            _ if arg.starts_with("--timeout=") => {
                let value = arg.strip_prefix("--timeout=").unwrap_or_default();
                timeout_secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("Error: invalid --timeout value `{value}`"))?;
            }
            _ if arg.starts_with('-') => return Err(format!("Error: unknown option `{arg}`")),
            _ => {
                if prompt.is_some() {
                    return Err(format!("Error: unexpected extra argument `{arg}`"));
                }
                prompt = Some(arg);
            }
        }
    }

    if workers.is_empty() {
        workers = workers_from_config_or_default(&config.swarm.default_workers, default_workers);
    }

    Ok(SwarmArgs {
        prompt: prompt.ok_or_else(|| "Error: missing prompt argument".to_string())?,
        cwd,
        timeout_secs,
        manager,
        workers,
        parent,
        slice,
        inject_context,
    })
}

pub fn parse_discuss_args<I>(raw: I) -> Result<DiscussArgs, String>
where
    I: IntoIterator<Item = String>,
{
    let config = crate::config::load_config();
    let mut prompt = None;
    let mut cwd = env::current_dir().map_err(|err| format!("Error reading cwd: {err}"))?;
    let mut timeout_secs = load_default_timeout();
    let mut manager = config
        .discussion
        .default_manager
        .as_deref()
        .and_then(|spec| parse_agent_spec_struct(spec).ok())
        .unwrap_or_else(default_manager);
    let mut participants = Vec::new();
    let mut rounds = config.discussion.default_rounds.unwrap_or(2);
    let mut parent = None;
    let mut slice = None;
    // None = "inherit config.settings.docs_default". Discuss has no built-in default.
    let mut docs: Option<bool> = None;
    let mut docs_agent = config
        .discussion
        .docs_agent
        .as_deref()
        .and_then(|spec| parse_agent_spec_struct(spec).ok())
        .unwrap_or_else(default_manager);
    let mut profile_helpers = false;

    let mut iter = raw.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help(timeout_secs);
                std::process::exit(0);
            }
            "--manager" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --manager requires a value".to_string())?;
                manager = parse_agent_spec_struct(&value)?;
            }
            "--participant" | "--worker" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("Error: {arg} requires a value"))?;
                participants.push(parse_worker_spec(&value)?);
            }
            "--parent" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --parent requires a value".to_string())?;
                parent = Some(value);
            }
            "--slice" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --slice requires a value".to_string())?;
                slice = Some(value);
            }
            "--rounds" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --rounds requires a value".to_string())?;
                rounds = value
                    .parse::<u32>()
                    .map_err(|_| format!("Error: invalid --rounds value `{value}`"))?;
            }
            "--docs" | "--api-docs" => docs = Some(true),
            "--no-docs" => docs = Some(false),
            "--helpers" | "--profile-helpers" => profile_helpers = true,
            "--no-helpers" => profile_helpers = false,
            "--docs-agent" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --docs-agent requires a value".to_string())?;
                docs_agent = parse_agent_spec_struct(&value)?;
            }
            "--cwd" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --cwd requires a value".to_string())?;
                cwd = PathBuf::from(value);
            }
            "--timeout" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --timeout requires a value".to_string())?;
                timeout_secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("Error: invalid --timeout value `{value}`"))?;
            }
            _ if arg.starts_with("--manager=") => {
                manager = parse_agent_spec_struct(arg.strip_prefix("--manager=").unwrap_or(""))?;
            }
            _ if arg.starts_with("--participant=") => {
                participants.push(parse_worker_spec(
                    arg.strip_prefix("--participant=").unwrap_or(""),
                )?);
            }
            _ if arg.starts_with("--worker=") => {
                participants.push(parse_worker_spec(
                    arg.strip_prefix("--worker=").unwrap_or(""),
                )?);
            }
            _ if arg.starts_with("--parent=") => {
                parent = Some(
                    arg.strip_prefix("--parent=")
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            _ if arg.starts_with("--slice=") => {
                slice = Some(arg.strip_prefix("--slice=").unwrap_or_default().to_string());
            }
            _ if arg.starts_with("--rounds=") => {
                let value = arg.strip_prefix("--rounds=").unwrap_or_default();
                rounds = value
                    .parse::<u32>()
                    .map_err(|_| format!("Error: invalid --rounds value `{value}`"))?;
            }
            _ if arg.starts_with("--docs-agent=") => {
                docs_agent =
                    parse_agent_spec_struct(arg.strip_prefix("--docs-agent=").unwrap_or(""))?;
            }
            _ if arg.starts_with("--cwd=") => {
                cwd = PathBuf::from(arg.strip_prefix("--cwd=").unwrap_or_default());
            }
            _ if arg.starts_with("--timeout=") => {
                let value = arg.strip_prefix("--timeout=").unwrap_or_default();
                timeout_secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("Error: invalid --timeout value `{value}`"))?;
            }
            _ if arg.starts_with('-') => return Err(format!("Error: unknown option `{arg}`")),
            _ => {
                if prompt.is_some() {
                    return Err(format!("Error: unexpected extra argument `{arg}`"));
                }
                prompt = Some(arg);
            }
        }
    }

    if participants.is_empty() {
        participants = workers_from_config_or_default(
            &config.discussion.default_participants,
            default_discussion_participants,
        );
    }

    Ok(DiscussArgs {
        prompt: prompt.ok_or_else(|| "Error: missing prompt argument".to_string())?,
        cwd,
        timeout_secs,
        manager,
        participants,
        rounds,
        parent,
        slice,
        docs,
        docs_agent,
        profile_helpers,
    })
}

pub fn parse_audit_args<I>(raw: I) -> Result<DiscussArgs, String>
where
    I: IntoIterator<Item = String>,
{
    let config = crate::config::load_config();
    let mut prompt = None;
    let mut focus = "all".to_string();
    let mut cwd = env::current_dir().map_err(|err| format!("Error reading cwd: {err}"))?;
    let mut timeout_secs = load_default_timeout();
    let mut manager = config
        .discussion
        .default_manager
        .as_deref()
        .and_then(|spec| parse_agent_spec_struct(spec).ok())
        .unwrap_or_else(default_manager);
    let mut participants = Vec::new();
    let mut rounds = config.discussion.default_rounds.unwrap_or(2);
    // Audit defaults docs ON (Some(true)). --no-docs flips to Some(false).
    // This hard-coded Some(true) means audit ignores config.settings.docs_default
    // (which only affects discuss/design where the default is None).
    let mut docs: Option<bool> = Some(true);
    let mut docs_agent = config
        .discussion
        .docs_agent
        .as_deref()
        .and_then(|spec| parse_agent_spec_struct(spec).ok())
        .unwrap_or_else(default_manager);
    let mut profile_helpers = false;

    let mut iter = raw.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help(timeout_secs);
                std::process::exit(0);
            }
            "--focus" => {
                focus = iter
                    .next()
                    .ok_or_else(|| "Error: --focus requires a value".to_string())?;
            }
            "--manager" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --manager requires a value".to_string())?;
                manager = parse_agent_spec_struct(&value)?;
            }
            "--participant" | "--worker" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("Error: {arg} requires a value"))?;
                participants.push(parse_worker_spec(&value)?);
            }
            "--rounds" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --rounds requires a value".to_string())?;
                rounds = value
                    .parse::<u32>()
                    .map_err(|_| format!("Error: invalid --rounds value `{value}`"))?;
            }
            "--docs" | "--api-docs" => docs = Some(true),
            "--no-docs" => docs = Some(false),
            "--helpers" | "--profile-helpers" => profile_helpers = true,
            "--no-helpers" => profile_helpers = false,
            "--docs-agent" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --docs-agent requires a value".to_string())?;
                docs_agent = parse_agent_spec_struct(&value)?;
            }
            "--cwd" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --cwd requires a value".to_string())?;
                cwd = PathBuf::from(value);
            }
            "--timeout" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --timeout requires a value".to_string())?;
                timeout_secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("Error: invalid --timeout value `{value}`"))?;
            }
            _ if arg.starts_with("--focus=") => {
                focus = arg.strip_prefix("--focus=").unwrap_or_default().to_string();
            }
            _ if arg.starts_with("--manager=") => {
                manager = parse_agent_spec_struct(arg.strip_prefix("--manager=").unwrap_or(""))?;
            }
            _ if arg.starts_with("--participant=") => {
                participants.push(parse_worker_spec(
                    arg.strip_prefix("--participant=").unwrap_or(""),
                )?);
            }
            _ if arg.starts_with("--worker=") => {
                participants.push(parse_worker_spec(
                    arg.strip_prefix("--worker=").unwrap_or(""),
                )?);
            }
            _ if arg.starts_with("--rounds=") => {
                let value = arg.strip_prefix("--rounds=").unwrap_or_default();
                rounds = value
                    .parse::<u32>()
                    .map_err(|_| format!("Error: invalid --rounds value `{value}`"))?;
            }
            _ if arg.starts_with("--docs-agent=") => {
                docs_agent =
                    parse_agent_spec_struct(arg.strip_prefix("--docs-agent=").unwrap_or(""))?;
            }
            _ if arg.starts_with("--cwd=") => {
                cwd = PathBuf::from(arg.strip_prefix("--cwd=").unwrap_or_default());
            }
            _ if arg.starts_with("--timeout=") => {
                let value = arg.strip_prefix("--timeout=").unwrap_or_default();
                timeout_secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("Error: invalid --timeout value `{value}`"))?;
            }
            _ if arg.starts_with('-') => return Err(format!("Error: unknown option `{arg}`")),
            _ => {
                if prompt.is_some() {
                    return Err(format!("Error: unexpected extra argument `{arg}`"));
                }
                prompt = Some(arg);
            }
        }
    }

    if participants.is_empty() {
        participants = default_audit_participants();
    }
    let prompt = prompt.ok_or_else(|| "Error: missing prompt argument".to_string())?;
    let prompt = build_audit_prompt(&prompt, &focus, &cwd);

    Ok(DiscussArgs {
        prompt,
        cwd,
        timeout_secs,
        manager,
        participants,
        rounds,
        parent: None,
        slice: None,
        docs,
        docs_agent,
        profile_helpers,
    })
}

pub fn parse_design_args<I>(raw: I) -> Result<DiscussArgs, String>
where
    I: IntoIterator<Item = String>,
{
    let config = crate::config::load_config();
    let mut prompt = None;
    let mut focus = "all".to_string();
    let mut cwd = env::current_dir().map_err(|err| format!("Error reading cwd: {err}"))?;
    let mut timeout_secs = load_default_timeout();
    let mut manager = config
        .design
        .default_manager
        .as_deref()
        .or(config.discussion.default_manager.as_deref())
        .and_then(|spec| parse_agent_spec_struct(spec).ok())
        .unwrap_or_else(default_manager);
    let mut participants = Vec::new();
    let mut rounds = config
        .design
        .default_rounds
        .or(config.discussion.default_rounds)
        .unwrap_or(2);
    // None = "inherit config.settings.docs_default". Design has no built-in default.
    let mut docs: Option<bool> = None;
    let mut docs_agent = config
        .design
        .docs_agent
        .as_deref()
        .or(config.discussion.docs_agent.as_deref())
        .and_then(|spec| parse_agent_spec_struct(spec).ok())
        .unwrap_or_else(default_manager);
    let mut profile_helpers = false;

    let mut iter = raw.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help(timeout_secs);
                std::process::exit(0);
            }
            "--focus" => {
                focus = iter
                    .next()
                    .ok_or_else(|| "Error: --focus requires a value".to_string())?;
            }
            "--manager" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --manager requires a value".to_string())?;
                manager = parse_agent_spec_struct(&value)?;
            }
            "--participant" | "--worker" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("Error: {arg} requires a value"))?;
                participants.push(parse_worker_spec(&value)?);
            }
            "--rounds" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --rounds requires a value".to_string())?;
                rounds = value
                    .parse::<u32>()
                    .map_err(|_| format!("Error: invalid --rounds value `{value}`"))?;
            }
            "--docs" | "--api-docs" => docs = Some(true),
            "--no-docs" => docs = Some(false),
            "--helpers" | "--profile-helpers" => profile_helpers = true,
            "--no-helpers" => profile_helpers = false,
            "--docs-agent" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --docs-agent requires a value".to_string())?;
                docs_agent = parse_agent_spec_struct(&value)?;
            }
            "--cwd" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --cwd requires a value".to_string())?;
                cwd = PathBuf::from(value);
            }
            "--timeout" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --timeout requires a value".to_string())?;
                timeout_secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("Error: invalid --timeout value `{value}`"))?;
            }
            _ if arg.starts_with("--focus=") => {
                focus = arg.strip_prefix("--focus=").unwrap_or_default().to_string();
            }
            _ if arg.starts_with("--manager=") => {
                manager = parse_agent_spec_struct(arg.strip_prefix("--manager=").unwrap_or(""))?;
            }
            _ if arg.starts_with("--participant=") => {
                participants.push(parse_worker_spec(
                    arg.strip_prefix("--participant=").unwrap_or(""),
                )?);
            }
            _ if arg.starts_with("--worker=") => {
                participants.push(parse_worker_spec(
                    arg.strip_prefix("--worker=").unwrap_or(""),
                )?);
            }
            _ if arg.starts_with("--rounds=") => {
                let value = arg.strip_prefix("--rounds=").unwrap_or_default();
                rounds = value
                    .parse::<u32>()
                    .map_err(|_| format!("Error: invalid --rounds value `{value}`"))?;
            }
            _ if arg.starts_with("--docs-agent=") => {
                docs_agent =
                    parse_agent_spec_struct(arg.strip_prefix("--docs-agent=").unwrap_or(""))?;
            }
            _ if arg.starts_with("--cwd=") => {
                cwd = PathBuf::from(arg.strip_prefix("--cwd=").unwrap_or_default());
            }
            _ if arg.starts_with("--timeout=") => {
                let value = arg.strip_prefix("--timeout=").unwrap_or_default();
                timeout_secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("Error: invalid --timeout value `{value}`"))?;
            }
            _ if arg.starts_with('-') => return Err(format!("Error: unknown option `{arg}`")),
            _ => {
                if prompt.is_some() {
                    return Err(format!("Error: unexpected extra argument `{arg}`"));
                }
                prompt = Some(arg);
            }
        }
    }

    if participants.is_empty() {
        participants = workers_from_config_or_default(
            &config.design.default_participants,
            default_design_participants,
        );
    }
    let prompt = prompt.ok_or_else(|| "Error: missing prompt argument".to_string())?;
    let prompt = build_design_prompt(&prompt, &focus, &cwd);

    Ok(DiscussArgs {
        prompt,
        cwd,
        timeout_secs,
        manager,
        participants,
        rounds,
        parent: None,
        slice: None,
        docs,
        docs_agent,
        profile_helpers,
    })
}

pub fn print_help(default_timeout_secs: u64) {
    println!(
         "usage: agent-swarm [run] [--background] [--agent claude|codex|auto] [--model MODEL] [--persona NAME|--no-persona] [--cwd CWD] [--timeout SECONDS] [--quiet] prompt\n\
         usage: agent-swarm swarm [--manager AGENT[:MODEL]] [--worker ROLE=AGENT[:MODEL]]... [--parent ID] [--slice ID] [--cwd CWD] [--timeout SECONDS] prompt\n\
         usage: agent-swarm fanout [--manager AGENT[:MODEL]] [--worker ROLE=AGENT[:MODEL]]... [--parent ID] [--slice ID] [--cwd CWD] [--timeout SECONDS] prompt\n\
         usage: agent-swarm discuss [--participant ROLE=AGENT[:MODEL]]... [--manager AGENT[:MODEL]] [--rounds N] [--parent ID] [--slice ID] [--docs] [--helpers] prompt\n\
         usage: agent-swarm metadirector [--model MODEL] [--cwd CWD] [--timeout SECONDS] prompt\n\
         usage: agent-swarm audit [--focus all|simplify|harden|architecture|api-docs|tests] [--participant ROLE=AGENT[:MODEL]]... [--rounds N] [--docs|--no-docs] [--helpers] prompt\n\
         usage: agent-swarm design [--focus all|visual-system|motion|interaction|accessibility|implementation] [--participant ROLE=AGENT[:MODEL]]... [--rounds N] [--helpers] prompt\n\
         usage: agent-swarm status [JOB_ID]\n\
         usage: agent-swarm result [JOB_ID]\n\
         usage: agent-swarm cancel JOB_ID\n\
         usage: agent-swarm sessions\n\
         usage: agent-swarm runtime-processes [--json] [--cull]\n\
         usage: agent-swarm events SESSION_ID\n\
         usage: agent-swarm transcript SESSION_ID\n\
         usage: agent-swarm insights\n\
         usage: agent-swarm profiles\n\
         usage: agent-swarm hooks\n\
         usage: agent-swarm presets\n\
         usage: agent-swarm preset PRESET_ID [--cwd CWD] [--timeout SECONDS] \"<task>\"\n\
         usage: agent-swarm recommend [--classifier auto|deterministic|semantic] [--classifier-threshold N] [--mlx-endpoint URL] \"<task>\"\n\
         usage: agent-swarm eval-metadirector [--arm classifier|packet|all|session|quality] [--fixtures PATH] [--classifier auto|deterministic|semantic] [--classifier-threshold N] [--mlx-endpoint URL] [--packet-budget N] [--manager-prompt-limit N] [--rate-file PATH] [--ledger PATH] [--session PATH] [--quality PATH] [--trials N] [--no-write-summary]\n\
         usage: agent-swarm ledger add --id ID --intent TEXT [--owner AGENT] [--depends-on ID]... [--status STATUS] [--anchor TEXT] [--dir PATH]\n\
         usage: agent-swarm ledger list|working-set [--dir PATH]\n\
         usage: agent-swarm ledger set-status --id ID --status open|claimed|claimed_done|verified_done [--anchor TEXT] [--dir PATH]\n\
         usage: agent-swarm feedback --role ROLE --agent AGENT --outcome win|loss [--session SESSION_ID] [--note NOTE]\n\
         usage: agent-swarm proposals\n\
         usage: agent-swarm propose [--title TITLE] [--by AGENT] [--tag TAG]... \"<body>\"\n\
         usage: agent-swarm proposal-vote PROPOSAL_ID approve|reject|defer [VOTER] [RATIONALE]\n\
         usage: agent-swarm manifest\n\
         usage: agent-swarm mcp\n\
         usage: agent-swarm provider add ID --type TYPE [--name NAME] [--endpoint URL] [--models A,B] [--data-dir PATH]\n\
         usage: agent-swarm provider models [TYPE]\n\
         usage: agent-swarm provider list|remove ID|key set ID [--from-env VAR]|key check ID [--data-dir PATH]\n\
         usage: agent-swarm doctor [--data-dir PATH]\n\
         usage: agent-swarm antigravity-config ensure [--config PATH] [--mcp PATH]\n\n\
         Dispatch a prompt to a detected frontier partner agent.\n\n\
         positional arguments:\n\
           prompt             The prompt or instruction for the partner agent.\n\n\
         options:\n\
           -h, --help                  show this help message and exit\n\
           --background                Queue a local job and return immediately.\n\
           --agent claude|codex|auto   Partner backend (default from config, then claude).\n\
           --backend claude|codex|auto Alias for --agent.\n\
          --model MODEL               Backend model hint; used by Claude and Codex where supported.\n\
           --persona NAME              Wrap direct runs with a compact persona/preprompt (for example compact-manager).\n\
           --profile NAME              Alias for --persona; profile ids and roles are accepted.\n\
           --no-persona                Disable the configured direct persona for one run.\n\
           --cwd CWD                   Working directory context (default: current directory).\n\
           --timeout SECONDS           Max execution timeout in seconds (default: {default_timeout_secs}).\n\
           --quiet                     Consult mode: read-only where supported and no dispatch banner.\n\n\
           --allow-bypass-permissions  Allow Claude Code permission bypass in non-quiet mode.\n\n\
         job commands:\n\
           status [JOB_ID]             Show recent jobs or one job.\n\
           result [JOB_ID]             Print a job result; defaults to latest job.\n\
           cancel JOB_ID               Terminate a queued/running job.\n\
           sessions                    Show recent discussion sessions.\n\
           runtime-processes           List tracked live/lost session and background-job processes; --cull attempts stuck untracked cleanup.\n\
           events SESSION_ID           Print a discussion JSONL event stream.\n\
           transcript SESSION_ID       Print a discussion transcript.\n\
           insights                    Print lazy routing insights from agent telemetry.\n\
           profiles                    Print role profiles, helper agents, automation hooks, and checks.\n\
           hooks                       Print deterministic host-only automation hook catalog.\n\
           presets                     Print common swarm presets.\n\
           preset                      Execute a named common swarm preset.\n\
           recommend                   Recommend manager/participant specs for a task.\n\
           eval-metadirector           Score fixture routing/escalation readiness for thin defaults.\n\
           feedback                    Record explicit routing feedback for future recommendations.\n\
           proposals                   Print open learning-layer proposals and vote totals.\n\
           propose                     Record a proposal for future swarm behavior.\n\
           proposal-vote               Vote on a proposal as a user or agent.\n\
           audit                       Run a preset read-only codebase audit discussion.\n\
           metadirector                Run the large-context metadirector contract in consult mode.\n\
           design                      Run a design-centered product/UI review discussion.\n\
           manifest                    Print service package metadata as JSON.\n\
           mcp                         Serve Agent Swarm tools over MCP stdio.\n\
           antigravity-config          Configure Antigravity MCP integration (ensure)."
    );
}

pub fn load_default_timeout() -> u64 {
    let Some(home) = home_dir() else {
        return DEFAULT_TIMEOUT_SECS;
    };
    let path = config_path(&home);
    let Ok(content) = std::fs::read_to_string(path) else {
        return DEFAULT_TIMEOUT_SECS;
    };
    for line in content.lines() {
        let without_comment = line.split('#').next().unwrap_or("").trim();
        if let Some((key, rhs)) = without_comment.split_once('=') {
            if key.trim() != "default_timeout" {
                continue;
            }
            if let Ok(timeout) = rhs.trim().trim_matches('"').parse::<u64>() {
                return timeout;
            }
        }
    }
    DEFAULT_TIMEOUT_SECS
}

fn load_default_agent() -> AgentChoice {
    let Some(home) = home_dir() else {
        return AgentChoice::Claude;
    };
    let path = config_path(&home);
    let Ok(content) = std::fs::read_to_string(path) else {
        return AgentChoice::Claude;
    };
    for line in content.lines() {
        let without_comment = line.split('#').next().unwrap_or("").trim();
        if let Some((key, rhs)) = without_comment.split_once('=') {
            if key.trim() != "default_agent" {
                continue;
            }
            let value = rhs.trim().trim_matches('"').trim_matches('\'');
            if let Ok((agent, _model)) = parse_agent_spec(value) {
                return agent;
            }
        }
    }
    AgentChoice::Claude
}

pub fn config_path(home: &Path) -> PathBuf {
    let codex_path = home.join(".codex/skills/agent-swarm/config.toml");
    if running_inside_codex() && codex_path.exists() {
        return codex_path;
    }
    let claude_path = home.join(".claude/skills/agent-swarm/config.toml");
    if claude_path.exists() {
        return claude_path;
    }
    let legacy_codex_path = home.join(".codex/skills/gemini-partner/config.toml");
    if running_inside_codex() && legacy_codex_path.exists() {
        return legacy_codex_path;
    }
    let legacy_claude_path = home.join(".claude/skills/gemini-partner/config.toml");
    if legacy_claude_path.exists() {
        return legacy_claude_path;
    }
    codex_path
}

pub fn parse_agent_choice(value: &str) -> Result<AgentChoice, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "codex" | "openai" => Ok(AgentChoice::Codex),
        "claude" | "anthropic" => Ok(AgentChoice::Claude),
        "auto" => Ok(AgentChoice::Auto),
        other => Err(format!(
            "Error: invalid agent `{other}`. Expected codex, claude, or auto."
        )),
    }
}

pub fn parse_agent_spec(value: &str) -> Result<(AgentChoice, Option<String>), String> {
    let mut parts = value.splitn(2, ':');
    let agent = parse_agent_choice(parts.next().unwrap_or_default())?;
    let model = parts
        .next()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToString::to_string);
    Ok((agent, model))
}

/// Parse `NAME[:MODEL]` into an [`AgentSpec`]. A NAME that is not a built-in
/// is accepted as a config-defined backend id (a `[backend.<id>]` descriptor);
/// validity is checked at dispatch against the `BackendRegistry`, whose
/// unknown-id error lists the available backends.
pub fn parse_agent_spec_struct(value: &str) -> Result<AgentSpec, String> {
    match parse_agent_spec(value) {
        Ok((agent, model)) => Ok(AgentSpec::builtin(agent, model)),
        Err(builtin_err) => {
            let mut parts = value.splitn(2, ':');
            let id = parts.next().unwrap_or_default().trim();
            if id.is_empty() || id.chars().any(char::is_whitespace) {
                return Err(builtin_err);
            }
            let model = parts
                .next()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(ToString::to_string);
            Ok(AgentSpec::for_custom(id, model))
        }
    }
}

pub fn parse_worker_spec(value: &str) -> Result<WorkerSpec, String> {
    let (base, modifiers) = value.split_once('@').unwrap_or((value, ""));
    let mut worker = if let Some((role, spec)) = base.split_once('=') {
        let role = role.trim();
        if role.is_empty() {
            return Err("Error: worker role cannot be empty".to_string());
        }
        WorkerSpec {
            role: role.to_string(),
            spec: parse_agent_spec_struct(spec.trim())?,
            timeout_secs: None,
        }
    } else {
        let parts: Vec<&str> = base.split(':').collect();
        if parts.len() < 2 || parse_agent_choice(parts[0]).is_err() {
            return Err(
                "Error: --worker must be ROLE=AGENT[:MODEL] or AGENT[:MODEL]:ROLE".to_string(),
            );
        }
        let agent = parse_agent_choice(parts[0])?;
        let (model, role) = if parts.len() >= 3 {
            (Some(parts[1].to_string()), parts[2])
        } else {
            (None, parts[1])
        };
        if role.trim().is_empty() {
            return Err("Error: worker role cannot be empty".to_string());
        }
        WorkerSpec {
            role: role.trim().to_string(),
            spec: AgentSpec::builtin(agent, model),
            timeout_secs: None,
        }
    };

    for modifier in modifiers
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let Some((key, value)) = modifier.split_once('=') else {
            return Err(format!("Error: invalid worker modifier `{modifier}`"));
        };
        match key.trim() {
            "timeout" | "timeout_secs" => {
                worker.timeout_secs = Some(
                    value
                        .trim()
                        .parse::<u64>()
                        .map_err(|_| format!("Error: invalid timeout modifier `{value}`"))?,
                );
            }
            other => return Err(format!("Error: unknown worker modifier `{other}`")),
        }
    }

    Ok(worker)
}

pub fn default_workers() -> Vec<WorkerSpec> {
    [
        ("architecture", "claude:sonnet"),
        ("implementation", "codex"),
        ("review", "claude:sonnet"),
    ]
    .into_iter()
    .filter_map(|(role, spec)| {
        let spec = parse_agent_spec_struct(spec).ok()?;
        if agent_available(spec.agent) {
            Some(WorkerSpec {
                role: role.to_string(),
                spec,
                timeout_secs: None,
            })
        } else {
            None
        }
    })
    .collect()
}

fn default_manager() -> AgentSpec {
    AgentSpec::builtin(AgentChoice::Claude, None)
}

fn workers_from_config_or_default(
    specs: &[String],
    fallback: fn() -> Vec<WorkerSpec>,
) -> Vec<WorkerSpec> {
    if specs.is_empty() {
        return fallback();
    }

    let workers = specs
        .iter()
        .filter_map(|spec| {
            let worker = parse_worker_spec(spec).ok()?;
            // Custom (config-defined) backends pass optimistically here —
            // their real availability check is registry resolve + ready() at
            // preflight/dispatch, which kernel staffing can't see.
            if worker.spec.custom.is_some() || agent_available(worker.spec.agent) {
                Some(worker)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    if workers.is_empty() {
        fallback()
    } else {
        workers
    }
}

pub fn default_discussion_participants() -> Vec<WorkerSpec> {
    [
        ("architecture", "claude:sonnet"),
        ("code-quality", "claude:sonnet"),
        ("implementation", "codex"),
    ]
    .into_iter()
    .filter_map(|(role, spec)| {
        let spec = parse_agent_spec_struct(spec).ok()?;
        if agent_available(spec.agent) {
            Some(WorkerSpec {
                role: role.to_string(),
                spec,
                timeout_secs: None,
            })
        } else {
            None
        }
    })
    .collect()
}

pub fn default_audit_participants() -> Vec<WorkerSpec> {
    [
        ("architecture", "claude:sonnet"),
        ("simplify", "claude:sonnet"),
        ("hardening", "claude:sonnet"),
    ]
    .into_iter()
    .filter_map(|(role, spec)| {
        let spec = parse_agent_spec_struct(spec).ok()?;
        if agent_available(spec.agent) {
            Some(WorkerSpec {
                role: role.to_string(),
                spec,
                timeout_secs: None,
            })
        } else {
            None
        }
    })
    .collect()
}

pub fn default_design_participants() -> Vec<WorkerSpec> {
    [
        ("product-design", "claude:sonnet"),
        ("interaction-motion", "claude:sonnet"),
        ("frontend-implementation", "codex"),
        ("accessibility-qa", "claude:sonnet"),
    ]
    .into_iter()
    .filter_map(|(role, spec)| {
        let spec = parse_agent_spec_struct(spec).ok()?;
        if agent_available(spec.agent) {
            Some(WorkerSpec {
                role: role.to_string(),
                spec,
                timeout_secs: None,
            })
        } else {
            None
        }
    })
    .collect()
}

pub fn parse_u64_arg(value: Option<&String>, label: &str) -> Result<u64, String> {
    value
        .ok_or_else(|| format!("Error: --{label} requires a value"))?
        .parse::<u64>()
        .map_err(|_| format!("Error: --{label} must be an integer"))
}

// ── Tests recovered from agent-swarm lib.rs (P5-S6) ─────────────────────────
//
// Originally lived in `tools/agent-swarm/rust/src/lib.rs mod tests` at
// commit 298202ad. S5 deleted lib.rs; tests were not relocated. S6 restores
// them here in `swarm-kernel::args` where their owners (`parse_args`,
// `parse_agent_choice`, `parse_agent_spec`, `parse_worker_spec`,
// `parse_swarm_args`, `parse_discuss_args`, `parse_audit_args`,
// `parse_design_args`) now live.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{describe_spec, AgentChoice, AgentSpec};

    #[test]
    fn agent_identity_labels_are_stable() {
        let cases = [
            (AgentChoice::Codex, "codex", "Codex", "codex"),
            (AgentChoice::Claude, "claude", "Claude", "claude"),
            (AgentChoice::Auto, "auto", "auto-selected agent", "agent"),
        ];
        for (choice, tracking_name, display_name, command_name) in cases {
            assert_eq!(crate::agent::agent_name(choice), tracking_name);
            assert_eq!(choice.display_name(), display_name);
            assert_eq!(choice.command_name(), command_name);
        }
        assert_eq!(
            describe_spec(&AgentSpec::builtin(
                AgentChoice::Claude,
                Some("sonnet".to_string())
            )),
            "claude:sonnet"
        );
        assert_eq!(
            describe_spec(&AgentSpec::builtin(AgentChoice::Codex, None)),
            "codex"
        );
    }

    #[test]
    fn parses_custom_backend_specs() {
        // An unknown name is accepted as a config-defined backend id;
        // validity is the registry's job at dispatch, not the parser's.
        let spec = parse_agent_spec_struct("my-backend").unwrap();
        assert_eq!(spec.custom.as_deref(), Some("my-backend"));
        assert_eq!(spec.model, None);
        assert_eq!(spec.backend_id(), "my-backend");

        let spec = parse_agent_spec_struct("api:gpt-5.4-mini").unwrap();
        assert_eq!(spec.custom.as_deref(), Some("api"));
        assert_eq!(spec.model.as_deref(), Some("gpt-5.4-mini"));
        assert_eq!(describe_spec(&spec), "api:gpt-5.4-mini");

        // Built-ins still parse as built-ins, never as custom.
        let spec = parse_agent_spec_struct("claude:sonnet").unwrap();
        assert_eq!(spec.agent, AgentChoice::Claude);
        assert!(spec.custom.is_none());

        // Garbage stays rejected at parse time.
        assert!(parse_agent_spec_struct("").is_err());
        assert!(parse_agent_spec_struct("has space").is_err());
    }

    #[test]
    fn agent_flag_accepts_custom_backend_id() {
        let args = parse_args([
            "--no-persona".to_string(),
            "--agent".to_string(),
            "local-echo:tiny".to_string(),
            "hi".to_string(),
        ])
        .unwrap();
        assert_eq!(args.agent_custom.as_deref(), Some("local-echo"));
        assert_eq!(args.model.as_deref(), Some("tiny"));

        // A built-in --agent leaves the custom lane empty.
        let args = parse_args([
            "--no-persona".to_string(),
            "--agent".to_string(),
            "codex".to_string(),
            "hi".to_string(),
        ])
        .unwrap();
        assert_eq!(args.agent, AgentChoice::Codex);
        assert!(args.agent_custom.is_none());
    }

    #[test]
    fn parses_basic_consult_args() {
        let args = parse_args([
            "--quiet".to_string(),
            "--cwd".to_string(),
            "/tmp/work".to_string(),
            "--timeout".to_string(),
            "42".to_string(),
            "--agent".to_string(),
            "claude:sonnet".to_string(),
            "--background".to_string(),
            "--no-persona".to_string(),
            "hello".to_string(),
        ])
        .unwrap();

        assert_eq!(args.prompt, "hello");
        assert_eq!(args.cwd, std::path::PathBuf::from("/tmp/work"));
        assert_eq!(args.timeout_secs, 42);
        assert!(args.quiet);
        assert_eq!(args.agent, AgentChoice::Claude);
        assert_eq!(args.model.as_deref(), Some("sonnet"));
        assert!(args.persona.is_none());
        assert!(args.background);
    }

    #[test]
    fn parses_direct_persona_aliases() {
        let args = parse_args([
            "--persona".to_string(),
            "compact-manager".to_string(),
            "hello".to_string(),
        ])
        .unwrap();
        assert_eq!(args.persona.as_deref(), Some("compact-manager"));

        let args =
            parse_args(["--persona=gemini-manager".to_string(), "hello".to_string()]).unwrap();
        assert_eq!(args.persona.as_deref(), Some("gemini-manager"));
        // Manager personas don't override the agent; it stays at the default.
        assert_eq!(args.agent, AgentChoice::Claude);

        let args = parse_args([
            "--profile=systems-architect".to_string(),
            "--no-profile".to_string(),
            "hello".to_string(),
        ])
        .unwrap();
        assert!(args.persona.is_none());
    }

    #[test]
    fn profile_persona_sets_default_agent_unless_agent_explicit() {
        let args = parse_args([
            "--persona".to_string(),
            "code-simplifier".to_string(),
            "hello".to_string(),
        ])
        .unwrap();
        assert_eq!(args.agent, AgentChoice::Claude);
        assert_eq!(args.model.as_deref(), Some("sonnet"));

        let args = parse_args([
            "--persona".to_string(),
            "gemini-large-context-manager".to_string(),
            "hello".to_string(),
        ])
        .unwrap();
        // Manager personas leave the agent at the default rather than overriding it.
        assert_eq!(args.agent, AgentChoice::Claude);
        assert!(args.model.is_none());

        let args = parse_args([
            "--agent".to_string(),
            "codex".to_string(),
            "--persona".to_string(),
            "code-simplifier".to_string(),
            "hello".to_string(),
        ])
        .unwrap();
        // An explicit --agent wins over the persona's default_agent.
        assert_eq!(args.agent, AgentChoice::Codex);
        assert!(args.model.is_none());
    }

    #[test]
    fn rejects_unknown_direct_persona_at_parse_time() {
        let err = parse_args([
            "--persona".to_string(),
            "made-up-persona".to_string(),
            "hello".to_string(),
        ])
        .unwrap_err();
        assert!(err.contains("unknown direct persona"));
    }

    #[test]
    fn rejects_extra_prompt_argument() {
        let err = parse_args(["one".to_string(), "two".to_string()]).unwrap_err();
        assert!(err.contains("unexpected extra argument"));
    }

    #[test]
    fn supports_dash_prefixed_prompt_after_separator() {
        let args = parse_args(["--".to_string(), "-dash prompt".to_string()]).unwrap();
        assert_eq!(args.prompt, "-dash prompt");
    }

    #[test]
    fn parses_agent_aliases() {
        assert_eq!(parse_agent_choice("codex").unwrap(), AgentChoice::Codex);
        assert_eq!(parse_agent_choice("openai").unwrap(), AgentChoice::Codex);
        assert_eq!(parse_agent_choice("claude").unwrap(), AgentChoice::Claude);
        assert_eq!(
            parse_agent_choice("anthropic").unwrap(),
            AgentChoice::Claude
        );
        assert_eq!(parse_agent_choice("auto").unwrap(), AgentChoice::Auto);
        assert!(parse_agent_choice("unknown").is_err());
    }

    #[test]
    fn parses_agent_specs_with_models() {
        let (agent, model) = parse_agent_spec("claude:opus").unwrap();
        assert_eq!(agent, AgentChoice::Claude);
        assert_eq!(model.as_deref(), Some("opus"));
    }

    #[test]
    fn parses_discussion_args_with_docs() {
        let args = parse_discuss_args([
            "--rounds".to_string(),
            "3".to_string(),
            "--participant".to_string(),
            "architecture=codex".to_string(),
            "--participant".to_string(),
            "docs=claude:sonnet".to_string(),
            "--docs".to_string(),
            "--docs-agent".to_string(),
            "claude:opus".to_string(),
            "audit this API".to_string(),
        ])
        .unwrap();

        assert_eq!(args.prompt, "audit this API");
        assert_eq!(args.rounds, 3);
        assert_eq!(args.participants.len(), 2);
        assert_eq!(args.docs, Some(true));
        assert_eq!(args.docs_agent.agent, AgentChoice::Claude);
        assert_eq!(args.docs_agent.model.as_deref(), Some("opus"));
    }

    #[test]
    fn parses_audit_args_with_focus() {
        let args = parse_audit_args([
            "--focus".to_string(),
            "harden".to_string(),
            "--rounds".to_string(),
            "1".to_string(),
            "--no-docs".to_string(),
            "review this crate".to_string(),
        ])
        .unwrap();

        assert!(args.prompt.contains("Focus: harden."));
        assert!(args.prompt.contains("review this crate"));
        assert_eq!(args.rounds, 1);
        assert_eq!(args.docs, Some(false));
    }

    #[test]
    fn parses_design_args_with_focus() {
        let args = parse_design_args([
            "--focus".to_string(),
            "motion".to_string(),
            "--rounds".to_string(),
            "1".to_string(),
            "polish the graph view".to_string(),
        ])
        .unwrap();

        assert!(args.prompt.contains("Focus: motion."));
        assert!(args.prompt.contains("technical operators"));
        assert!(args.prompt.contains("polish the graph view"));
        assert_eq!(args.rounds, 1);
        assert_eq!(args.docs, None);
    }

    #[test]
    fn parses_worker_timeout_modifier() {
        let worker = parse_worker_spec("review=claude:sonnet@timeout=90").unwrap();
        assert_eq!(worker.role, "review");
        assert_eq!(worker.spec.agent, AgentChoice::Claude);
        assert_eq!(worker.spec.model.as_deref(), Some("sonnet"));
        assert_eq!(worker.timeout_secs, Some(90));
    }

    #[test]
    fn parse_swarm_args_inject_context_defaults_none() {
        // No --context / --no-context flag: inject_context is None (use config).
        let args = parse_swarm_args(
            [
                "--manager",
                "claude",
                "--worker",
                "qa=codex",
                "do the thing",
            ]
            .iter()
            .map(|s| s.to_string()),
        )
        .unwrap();
        assert_eq!(args.inject_context, None);
    }

    #[test]
    fn parse_swarm_args_context_flag_sets_true() {
        let args = parse_swarm_args(
            ["--context", "--worker", "qa=codex", "do the thing"]
                .iter()
                .map(|s| s.to_string()),
        )
        .unwrap();
        assert_eq!(args.inject_context, Some(true));
    }

    #[test]
    fn parse_swarm_args_no_context_flag_sets_false() {
        let args = parse_swarm_args(
            ["--no-context", "--worker", "qa=codex", "do the thing"]
                .iter()
                .map(|s| s.to_string()),
        )
        .unwrap();
        assert_eq!(args.inject_context, Some(false));
    }

    #[test]
    fn parses_swarm_parent_and_slice_flags() {
        let args = parse_swarm_args(
            [
                "--parent",
                "session-parent-1",
                "--slice",
                "slice-a",
                "--worker",
                "qa=codex",
                "run it",
            ]
            .iter()
            .map(|s| s.to_string()),
        )
        .unwrap();

        assert_eq!(args.parent.as_deref(), Some("session-parent-1"));
        assert_eq!(args.slice.as_deref(), Some("slice-a"));
    }

    #[test]
    fn parses_discuss_parent_and_slice_flags() {
        let args = parse_discuss_args(
            [
                "--parent=session-parent-2",
                "--slice=slice-b",
                "--participant",
                "architecture=claude",
                "review it",
            ]
            .iter()
            .map(|s| s.to_string()),
        )
        .unwrap();

        assert_eq!(args.parent.as_deref(), Some("session-parent-2"));
        assert_eq!(args.slice.as_deref(), Some("slice-b"));
    }
}
