#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CliCommand {
    Status,
    Result,
    Cancel,
    Manifest,
    Insights,
    Profiles,
    Hooks,
    AutomationHooks,
    Presets,
    Recommend,
    Feedback,
    Proposals,
    Propose,
    ProposalVote,
    Preset,
    EvalMetadirector,
    Ledger,
    Monitor,
    MonitorOnce,
    MonitorStart,
    MonitorStatus,
    Alerts,
    Watch,
    Mcp,
    Swarm,
    Fanout,
    Discuss,
    Metadirector,
    Design,
    Audit,
    Sessions,
    RuntimeProcesses,
    RuntimeProcessesUnderscore,
    Events,
    Transcript,
    ConductorHook,
    ActivityRecord,
    JobWorker,
    CommandWorker,
    Run,
    Overview,
    Provider,
    Doctor,
    AntigravityConfig,
    ScaffoldBackend,
}

const CLI_COMMANDS: &[(&str, CliCommand)] = &[
    ("status", CliCommand::Status),
    ("result", CliCommand::Result),
    ("cancel", CliCommand::Cancel),
    ("manifest", CliCommand::Manifest),
    ("insights", CliCommand::Insights),
    ("profiles", CliCommand::Profiles),
    ("hooks", CliCommand::Hooks),
    ("automation-hooks", CliCommand::AutomationHooks),
    ("presets", CliCommand::Presets),
    ("recommend", CliCommand::Recommend),
    ("feedback", CliCommand::Feedback),
    ("proposals", CliCommand::Proposals),
    ("propose", CliCommand::Propose),
    ("proposal-vote", CliCommand::ProposalVote),
    ("preset", CliCommand::Preset),
    ("eval-metadirector", CliCommand::EvalMetadirector),
    ("ledger", CliCommand::Ledger),
    ("monitor", CliCommand::Monitor),
    ("monitor-once", CliCommand::MonitorOnce),
    ("monitor-start", CliCommand::MonitorStart),
    ("monitor-status", CliCommand::MonitorStatus),
    ("alerts", CliCommand::Alerts),
    ("watch", CliCommand::Watch),
    ("mcp", CliCommand::Mcp),
    ("swarm", CliCommand::Swarm),
    ("fanout", CliCommand::Fanout),
    ("discuss", CliCommand::Discuss),
    ("metadirector", CliCommand::Metadirector),
    ("design", CliCommand::Design),
    ("audit", CliCommand::Audit),
    ("sessions", CliCommand::Sessions),
    ("runtime-processes", CliCommand::RuntimeProcesses),
    ("runtime_processes", CliCommand::RuntimeProcessesUnderscore),
    ("events", CliCommand::Events),
    ("transcript", CliCommand::Transcript),
    ("conductor-hook", CliCommand::ConductorHook),
    ("activity-record", CliCommand::ActivityRecord),
    ("__job-worker", CliCommand::JobWorker),
    ("__command-worker", CliCommand::CommandWorker),
    ("run", CliCommand::Run),
    ("overview", CliCommand::Overview),
    ("provider", CliCommand::Provider),
    ("doctor", CliCommand::Doctor),
    ("antigravity-config", CliCommand::AntigravityConfig),
    ("scaffold-backend", CliCommand::ScaffoldBackend),
];

impl CliCommand {
    pub(crate) fn parse(token: &str) -> Option<Self> {
        CLI_COMMANDS
            .iter()
            .find_map(|(candidate, command)| (*candidate == token).then_some(*command))
    }
}

// ── CLI surface stability guard ────────────────────────────────────────────────
//
// This test enforces byte-identical CLI surface — the skill wrappers that drive
// agent-swarm depend on these exact subcommand tokens. Any token added/removed/
// renamed MUST update this list explicitly (not by accident).

#[cfg(test)]
pub(crate) fn cli_subcommand_tokens() -> Vec<&'static str> {
    CLI_COMMANDS.iter().map(|(token, _)| *token).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn cli_subcommand_token_contract_is_stable() {
        let tokens = cli_subcommand_tokens();
        assert_eq!(
            tokens,
            vec![
                "status",
                "result",
                "cancel",
                "manifest",
                "insights",
                "profiles",
                "hooks",
                "automation-hooks",
                "presets",
                "recommend",
                "feedback",
                "proposals",
                "propose",
                "proposal-vote",
                "preset",
                "eval-metadirector",
                "ledger",
                "monitor",
                "monitor-once",
                "monitor-start",
                "monitor-status",
                "alerts",
                "watch",
                "mcp",
                "swarm",
                "fanout",
                "discuss",
                "metadirector",
                "design",
                "audit",
                "sessions",
                "runtime-processes",
                "runtime_processes",
                "events",
                "transcript",
                "conductor-hook",
                "activity-record",
                "__job-worker",
                "__command-worker",
                "run",
                "overview",
                "provider",
                "doctor",
                "antigravity-config",
                "scaffold-backend",
            ]
        );
        let unique = tokens.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(unique.len(), tokens.len());
        for token in &tokens {
            assert!(
                CliCommand::parse(token).is_some(),
                "token {token} must dispatch"
            );
        }
        assert_eq!(
            CliCommand::parse("runtime_processes"),
            Some(CliCommand::RuntimeProcessesUnderscore)
        );
        assert_eq!(CliCommand::parse("unknown-command"), None);
    }
}
