use std::env;
use std::io::{self, IsTerminal, Read};
use std::path::Path;
use std::time::Duration;

use crate::cli::CliCommand;
use crate::cli_commands::{
    cmd_alerts, cmd_antigravity_config, cmd_cancel, cmd_result, cmd_runtime_processes,
    cmd_session_events, cmd_session_transcript, cmd_sessions, cmd_status,
};
use crate::cli_read_commands::{
    cmd_activity_record, cmd_automation_hooks, cmd_conductor_hook, cmd_eval_metadirector,
    cmd_feedback, cmd_insights, cmd_ledger, cmd_manifest, cmd_overview, cmd_presets, cmd_profiles,
    cmd_proposal_vote, cmd_proposals, cmd_propose, cmd_recommend,
};
use crate::scaffold::cmd_scaffold_backend;
use swarm_exec::background_runtime::{cmd_command_worker, cmd_job_worker, start_background_job};
use swarm_exec::executor::current_swarm_depth;
use swarm_exec::monitor_runtime::{
    cmd_monitor, cmd_monitor_once, cmd_monitor_start, cmd_monitor_status, cmd_watch,
};
use swarm_exec::orchestration::{cmd_preset, run_discussion, run_partner_foreground, run_swarm};
use swarm_exec::synthesis::build_direct_persona_prompt;
use swarm_kernel::agent::agent_name;
use swarm_kernel::args::{
    parse_args, parse_audit_args, parse_design_args, parse_discuss_args, parse_swarm_args,
    print_help, Args, DEFAULT_TIMEOUT_SECS,
};
use swarm_kernel::format::prompt_preview;
use swarm_kernel::job_types::{JobAgent, JobMode, JobStatus};
use swarm_kernel::process::stdin_ready;
use swarm_kernel::resolver::resolve_agent;
use swarm_mcp::mcp_dispatch::cmd_mcp;
use swarm_mcp::mcp_helpers::invoked_as_mcp_binary;
use swarm_store::repos::job_repo::{FileJobRepo, JobRepo, JobSpec};
use swarm_store::store::{job_store_dir, now_ms, write_text_atomic};

const MAX_PROMPT_BYTES: usize = 180_000;
const MAX_SWARM_DEPTH: u32 = 3;
const EXIT_DEPTH_LIMIT_EXCEEDED: i32 = 124;

#[derive(Default)]
pub struct SwarmService;

impl SwarmService {
    pub fn new() -> Self {
        Self
    }

    pub fn run(&self) -> Result<i32, String> {
        if current_swarm_depth() >= MAX_SWARM_DEPTH {
            eprintln!(
                "Error: refusing to spawn agent-swarm beyond depth {}. Set by SWARM_DEPTH.",
                MAX_SWARM_DEPTH
            );
            return Ok(EXIT_DEPTH_LIMIT_EXCEEDED);
        }
        let raw: Vec<String> = env::args().skip(1).collect();
        if raw
            .iter()
            .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
        {
            print_help(DEFAULT_TIMEOUT_SECS);
            return Ok(0);
        }
        if raw.is_empty() && invoked_as_mcp_binary() {
            return cmd_mcp();
        }
        if let Some(command) = raw.first().and_then(|token| CliCommand::parse(token)) {
            match command {
                CliCommand::Status => return cmd_status(&raw[1..]),
                CliCommand::Result => return cmd_result(&raw[1..]),
                CliCommand::Cancel => return cmd_cancel(&raw[1..]),
                CliCommand::Manifest => return cmd_manifest(),
                CliCommand::Insights => return cmd_insights(),
                CliCommand::Profiles => return cmd_profiles(),
                CliCommand::Hooks | CliCommand::AutomationHooks => return cmd_automation_hooks(),
                CliCommand::Presets => return cmd_presets(),
                CliCommand::Recommend => return cmd_recommend(&raw[1..]),
                CliCommand::Feedback => return cmd_feedback(&raw[1..]),
                CliCommand::Proposals => return cmd_proposals(),
                CliCommand::Propose => return cmd_propose(&raw[1..]),
                CliCommand::ProposalVote => return cmd_proposal_vote(&raw[1..]),
                CliCommand::Preset => return cmd_preset(&raw[1..]),
                CliCommand::EvalMetadirector => return cmd_eval_metadirector(&raw[1..]),
                CliCommand::Ledger => return cmd_ledger(&raw[1..]),
                CliCommand::Monitor => return cmd_monitor(&raw[1..]),
                CliCommand::MonitorOnce => return cmd_monitor_once(&raw[1..]),
                CliCommand::MonitorStart => return cmd_monitor_start(&raw[1..]),
                CliCommand::MonitorStatus => return cmd_monitor_status(),
                CliCommand::Alerts => return cmd_alerts(&raw[1..]),
                CliCommand::Watch => return cmd_watch(&raw[1..]),
                CliCommand::Mcp => return cmd_mcp(),
                CliCommand::Swarm | CliCommand::Fanout => {
                    let args = parse_swarm_args(raw.into_iter().skip(1))?;
                    return run_swarm(args);
                }
                CliCommand::Discuss => {
                    let args = parse_discuss_args(raw.into_iter().skip(1))?;
                    return run_discussion(args);
                }
                CliCommand::Metadirector => {
                    let mut rewritten = vec![
                        "--agent".to_string(),
                        "gemini".to_string(),
                        "--persona".to_string(),
                        "gemini-large-context-manager".to_string(),
                        "--quiet".to_string(),
                    ];
                    rewritten.extend(raw.into_iter().skip(1));
                    let args = parse_args(rewritten)?;
                    return self.run_dispatch(args);
                }
                CliCommand::Design => {
                    let args = parse_design_args(raw.into_iter().skip(1))?;
                    return run_discussion(args);
                }
                CliCommand::Audit => {
                    let args = parse_audit_args(raw.into_iter().skip(1))?;
                    return run_discussion(args);
                }
                CliCommand::Sessions => return cmd_sessions(),
                CliCommand::RuntimeProcesses | CliCommand::RuntimeProcessesUnderscore => {
                    return cmd_runtime_processes(&raw[1..]);
                }
                CliCommand::Events => return cmd_session_events(&raw[1..]),
                CliCommand::Transcript => return cmd_session_transcript(&raw[1..]),
                CliCommand::ConductorHook => return cmd_conductor_hook(),
                CliCommand::ActivityRecord => return cmd_activity_record(&raw[1..]),
                CliCommand::JobWorker => return cmd_job_worker(&raw[1..]),
                CliCommand::CommandWorker => return cmd_command_worker(&raw[1..]),
                CliCommand::Run => {
                    let args = parse_args(raw.into_iter().skip(1))?;
                    return self.run_dispatch(args);
                }
                CliCommand::Overview => return cmd_overview(),
                CliCommand::AntigravityConfig => return cmd_antigravity_config(&raw[1..]),
                CliCommand::ScaffoldBackend => return cmd_scaffold_backend(&raw[1..]),
            }
        }

        let args = parse_args(raw)?;
        self.run_dispatch(args)
    }

    pub fn run_dispatch(&self, args: Args) -> Result<i32, String> {
        let mut prompt = args.prompt.clone();
        let job_repo = FileJobRepo::new(job_store_dir()?);
        let mut tracking = if args.background {
            None
        } else {
            let agent = resolve_agent(args.agent)?;
            Some(
                job_repo
                    .create(JobSpec {
                        agent: JobAgent::from_agent_name(agent_name(agent)),
                        model: args.model.clone(),
                        mode: if args.quiet {
                            JobMode::Consult
                        } else {
                            JobMode::Agent
                        },
                        cwd: args.cwd.clone(),
                        prompt_preview: prompt_preview(&args.prompt),
                        prompt_text: prompt.clone(),
                        timeout_secs: args.timeout_secs,
                        allow_recursive_codex: false,
                    })
                    .map_err(|e| e.to_string())?,
            )
        };

        if !io::stdin().is_terminal() && stdin_ready(Duration::from_millis(250)) {
            let mut stdin_text = String::new();
            io::stdin()
                .read_to_string(&mut stdin_text)
                .map_err(|err| format!("Error reading stdin: {err}"))?;
            if !stdin_text.trim().is_empty() {
                prompt = format!(
                    "Context:\n```\n{}\n```\n\n{}",
                    stdin_text.trim_end(),
                    args.prompt
                );
            }
        }
        if let Some(persona) = args.persona.as_deref() {
            prompt = build_direct_persona_prompt(&prompt, persona)?;
        }
        if let Some(record) = tracking.as_mut() {
            write_text_atomic(Path::new(&record.prompt_path), &prompt)?;
            record.prompt_preview = prompt_preview(&prompt);
            job_repo.save(record).map_err(|e| e.to_string())?;
        }

        let prompt_bytes = prompt.len();
        if prompt_bytes > MAX_PROMPT_BYTES {
            let message = format!(
                "Error: prompt is too large for the partner bridge ({prompt_bytes} bytes). \
                 Keep the prompt under {MAX_PROMPT_BYTES} bytes or summarize the context first."
            );
            eprintln!("{message}");
            if let Some(record) = tracking.as_mut() {
                write_text_atomic(Path::new(&record.stdout_path), "")?;
                write_text_atomic(Path::new(&record.stderr_path), &message)?;
                write_text_atomic(Path::new(&record.result_path), "")?;
                record.status = JobStatus::Failed;
                record.completed_at_ms = Some(now_ms());
                record.exit_code = Some(2);
                job_repo.save(record).map_err(|e| e.to_string())?;
            }
            return Ok(2);
        }

        if args.background {
            return start_background_job(args, prompt);
        }

        run_partner_foreground(&args, &prompt, tracking, &job_repo)
    }
}
