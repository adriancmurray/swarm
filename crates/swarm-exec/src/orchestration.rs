//! Swarm and discussion orchestration: subprocess fan-out, agent-turn
//! execution, preset expansion, and profile-helper runs.
//!
//! Extracted from `main.rs` (seam 27). `main()`/`run()`/`run_dispatch()` stay in
//! `main.rs` as the policy layer (depth/prompt guards, stdin handling, tracking
//! lifecycle, background-vs-foreground decision); this module is the mechanism
//! layer they delegate to.

use std::io::{self, Write};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

use swarm_kernel::agent::{describe_spec, AgentSpec};
use swarm_kernel::args::{
    load_default_timeout, parse_agent_spec_struct, parse_audit_args, parse_design_args,
    parse_discuss_args, parse_swarm_args, print_help, Args, DiscussArgs, SwarmArgs, WorkerSpec,
};
use swarm_kernel::config::{load_config, resolve_docs, SwarmConfig};
use swarm_kernel::context::context_gather_json;
use swarm_kernel::events::EventKind;
use swarm_kernel::format::prompt_preview;
use swarm_kernel::ids::PresetId;
use swarm_kernel::job_types::{JobAgent, JobMode, JobStatus};
use swarm_kernel::profiles;
use swarm_kernel::routing::build_fallback_chain;
use swarm_store::job::JobRecord;
use swarm_store::repos::job_repo::{FileJobRepo, JobRepo, JobSpec};
use swarm_store::store::{job_store_dir, now_ms, write_text_atomic};

use crate::backend_registry::BackendRegistry;
use crate::executor::{
    execute_partner, execute_with_fallback, execute_with_fallback_chunks, output_record_status,
    output_status_code, print_partner_output, record_agent_error, record_agent_observation,
    FallbackOutcome,
};
use crate::preflight::{
    classified_agent_error_payload, classified_error_payload, run_session_preflight,
};
use crate::session::{DiscussionSession, DiscussionTurn};
use crate::synthesis::{
    build_discussion_digest, build_discussion_manager_prompt, build_discussion_turn_prompt,
    build_docs_prompt, build_manager_prompt, build_profile_helper_prompt,
    build_swarm_result_artifact, build_swarm_transcript, build_worker_prompt,
    capped_manager_output, preview_for_event, render_context_block,
};

pub fn run_partner_foreground(
    args: &Args,
    prompt: &str,
    mut tracking: Option<JobRecord>,
    job_repo: &FileJobRepo,
) -> Result<i32, String> {
    // Custom (config-defined) backends skip built-in resolution: their agent
    // field is a placeholder, and the registry validates the id at dispatch.
    let agent = match args.agent_custom {
        Some(_) => args.agent,
        None => swarm_kernel::resolver::resolve_agent(args.agent)?,
    };
    if !args.quiet {
        let display = args
            .agent_custom
            .clone()
            .unwrap_or_else(|| agent.display_name().to_string());
        println!(
            "Dispatching to {} (agent, timeout {}s)\n  cwd:    {}\n  prompt: {:?}\n{}",
            display,
            args.timeout_secs,
            args.cwd.display(),
            args.prompt,
            "-".repeat(60)
        );
        io::stdout().flush().ok();
    }
    let registry = BackendRegistry::from_config(&swarm_kernel::config::load_config());

    // Use the caller-provided repo so we share the same FileJobRepo instance
    // that was used to create the tracking record in run_dispatch.
    let finish_record =
        |record: &mut JobRecord, status, code, stdout: &str, stderr: &str, result: &str| {
            write_text_atomic(std::path::Path::new(&record.stdout_path), stdout)?;
            write_text_atomic(std::path::Path::new(&record.stderr_path), stderr)?;
            write_text_atomic(std::path::Path::new(&record.result_path), result)?;
            record.status = status;
            record.completed_at_ms = Some(now_ms());
            record.exit_code = Some(code);
            job_repo.save(record).map_err(|e| e.to_string())
        };

    match execute_partner(&registry, agent, args, prompt) {
        Ok(output) => {
            let code = output_status_code(&output);
            let status = output_record_status(&output, code);
            if let Some(record) = tracking.as_mut() {
                finish_record(
                    record,
                    status,
                    code,
                    &output.stdout,
                    &output.stderr,
                    &output.stdout,
                )?;
            }
            print_partner_output(agent, args, output)
        }
        Err(err) => {
            if let Some(record) = tracking.as_mut() {
                finish_record(record, JobStatus::Failed, 1, "", &err, "")?;
            }
            Err(err)
        }
    }
}

/// Emits a visible reliability event from a fallback outcome: `worker_fallback`
/// when the chain advanced past the requested backend, or `backend_retry` when
/// the requested backend succeeded only after a retry. A clean first-try success
/// emits nothing. Degradation is never silent.
fn emit_reliability_events(
    session: &DiscussionSession,
    role: &str,
    requested: &AgentSpec,
    outcome: &FallbackOutcome,
) {
    if outcome.fell_back() {
        let attempts: Vec<serde_json::Value> = outcome
            .attempts
            .iter()
            .map(|attempt| {
                serde_json::json!({
                    "agent": describe_spec(&attempt.spec),
                    "retries": attempt.retries,
                    "succeeded": attempt.succeeded,
                    "reason": attempt.reason,
                })
            })
            .collect();
        let _ = session.append_event(
            EventKind::WorkerFallback,
            serde_json::json!({
                "role": role,
                "requested": describe_spec(requested),
                "used": describe_spec(&outcome.used),
                "attempts": attempts,
            }),
        );
    } else if outcome
        .attempts
        .first()
        .map_or(0, |attempt| attempt.retries)
        > 0
    {
        let retries = outcome
            .attempts
            .first()
            .map_or(0, |attempt| attempt.retries);
        let _ = session.append_event(
            EventKind::BackendRetry,
            serde_json::json!({
                "role": role,
                "agent": describe_spec(&outcome.used),
                "retries": retries,
            }),
        );
    }
}

pub fn run_swarm(args: SwarmArgs) -> Result<i32, String> {
    if args.workers.is_empty() {
        return Err("Error: swarm requires at least one available worker".to_string());
    }

    let config = load_config();
    let session = DiscussionSession::create_swarm(&args)?;
    session.write_swarm_metadata(&args)?;
    session.append_event(
        EventKind::FanoutStarted,
        serde_json::json!({
            "prompt": &args.prompt,
            "cwd": args.cwd.display().to_string(),
            "manager": describe_spec(&args.manager),
            "workers": args.workers.iter().map(|worker| {
                serde_json::json!({
                    "role": &worker.role,
                    "agent": describe_spec(&worker.spec)
                })
            }).collect::<Vec<_>>()
        }),
    )?;
    if let Err(err) = run_session_preflight(
        &session,
        &BackendRegistry::from_config(&config),
        &args.manager,
        &args.workers,
    ) {
        session.append_event(
            EventKind::SessionCompleted,
            serde_json::json!({"failed": true}),
        )?;
        return Err(err);
    }

    let job_repo = FileJobRepo::new(job_store_dir().map_err(|e| e.to_string())?);
    let mut tracking = job_repo
        .create(JobSpec {
            agent: JobAgent::Swarm,
            model: Some(describe_spec(&args.manager)),
            mode: JobMode::Swarm,
            cwd: args.cwd.clone(),
            prompt_preview: prompt_preview(&args.prompt),
            prompt_text: args.prompt.clone(),
            timeout_secs: args.timeout_secs,
            allow_recursive_codex: false,
        })
        .map_err(|e| e.to_string())?;

    println!(
        "Starting agent swarm: manager={} workers={}",
        describe_spec(&args.manager),
        args.workers
            .iter()
            .map(|worker| format!("{}={}", worker.role, describe_spec(&worker.spec)))
            .collect::<Vec<_>>()
            .join(", ")
    );
    io::stdout().flush().ok();

    // Auto-context injection: gather once before spawning workers.
    // Effective flag = CLI override (args.inject_context) or config key.
    let effective_inject = args.inject_context.unwrap_or(config.context.auto_inject);
    let context_block: Option<String> = if effective_inject {
        match context_gather_json(&args.cwd, &args.prompt, 512) {
            Ok(ctx_json) => render_context_block(&ctx_json),
            Err(err) => {
                eprintln!(
                    "agent-swarm: warning: auto-context gather failed ({err}); proceeding without context"
                );
                None
            }
        }
    } else {
        None
    };
    // Render to an owned String so worker threads can share a borrow.
    let context_ref: Option<String> = context_block;

    let mut handles = Vec::new();
    for worker in args.workers.clone() {
        let prompt = build_worker_prompt(&args.prompt, &worker.role, context_ref.as_deref());
        let cwd = args.cwd.clone();
        let timeout_secs = worker.timeout_secs.unwrap_or(args.timeout_secs);
        let session = session.clone();
        let config = config.clone();
        handles.push(thread::spawn(move || {
            let _ = session.append_event(
                EventKind::WorkerStarted,
                serde_json::json!({
                    "role": &worker.role,
                    "agent": describe_spec(&worker.spec)
                }),
            );
            let call_args = Args {
                prompt: prompt.clone(),
                cwd,
                timeout_secs,
                quiet: true,
                agent: worker.spec.agent,
                agent_custom: worker.spec.custom.clone(),
                model: worker.spec.model.clone(),
                persona: None,
                background: false,
                allow_bypass_permissions: false,
            };
            let chain = build_fallback_chain(&worker.role, &worker.spec, &config);
            let started = Instant::now();
            let fallback = execute_with_fallback(
                &BackendRegistry::from_config(&config),
                &chain,
                &call_args,
                &prompt,
                &worker.role,
                &config.reliability,
            );
            emit_reliability_events(&session, &worker.role, &worker.spec, &fallback);
            let ran = fallback.used.clone();
            let output = fallback.result;
            match &output {
                Ok(output) => {
                    record_agent_observation(
                        "fanout-worker",
                        Some(session.id.as_str()),
                        &worker.role,
                        &ran,
                        &call_args.cwd,
                        &prompt,
                        output,
                        started.elapsed(),
                    );
                    let code = output_status_code(output);
                    let _ = session.append_event(
                        EventKind::WorkerCompleted,
                        serde_json::json!({
                            "role": &worker.role,
                            "agent": describe_spec(&ran),
                            "exit_code": code,
                            "timed_out": output.timed_out,
                            "text": output.stdout.trim(),
                            "stderr": output.stderr.trim()
                        }),
                    );
                    let _ = session.append_layer_report(
                        "worker",
                        &worker.role,
                        &describe_spec(&ran),
                        Some("manager"),
                        if output.timed_out || code != 0 {
                            "failed"
                        } else {
                            "completed"
                        },
                        output.stdout.trim(),
                    );
                }
                Err(err) => {
                    record_agent_error(
                        "fanout-worker",
                        Some(session.id.as_str()),
                        &worker.role,
                        &ran,
                        &call_args.cwd,
                        &prompt,
                        err,
                        started.elapsed(),
                    );
                    let _ = session.append_event(
                        EventKind::WorkerFailed,
                        serde_json::json!({
                            "role": &worker.role,
                            "agent": describe_spec(&ran),
                            "error": err
                        }),
                    );
                    let _ = session.append_layer_report(
                        "worker",
                        &worker.role,
                        &describe_spec(&ran),
                        Some("manager"),
                        "failed",
                        err,
                    );
                }
            }
            (worker, output)
        }));
    }

    let mut worker_results = Vec::new();
    let mut failed = false;
    for handle in handles {
        match handle.join() {
            Ok((worker, Ok(output))) => {
                let code = output_status_code(&output);
                if output.timed_out || code != 0 {
                    failed = true;
                }
                worker_results.push((worker, code, output));
            }
            Ok((worker, Err(err))) => {
                failed = true;
                eprintln!("worker {} failed: {err}", worker.role);
            }
            Err(_) => {
                failed = true;
                eprintln!("worker panicked");
            }
        }
    }

    let synthesis_prompt =
        build_manager_prompt(&args.prompt, &worker_results, context_ref.as_deref());
    let manager_args = Args {
        prompt: synthesis_prompt.clone(),
        cwd: args.cwd,
        timeout_secs: args.timeout_secs,
        quiet: true,
        agent: args.manager.agent,
        agent_custom: args.manager.custom.clone(),
        model: args.manager.model.clone(),
        persona: None,
        background: false,
        allow_bypass_permissions: false,
    };
    session.append_event(
        EventKind::ManagerStarted,
        serde_json::json!({
            "agent": describe_spec(&args.manager)
        }),
    )?;
    let manager_chain = build_fallback_chain("manager", &args.manager, &config);
    let manager_started = Instant::now();
    let manager_fallback = execute_with_fallback(
        &BackendRegistry::from_config(&config),
        &manager_chain,
        &manager_args,
        &synthesis_prompt,
        "manager",
        &config.reliability,
    );
    emit_reliability_events(&session, "manager", &args.manager, &manager_fallback);
    let manager_ran = manager_fallback.used.clone();
    let mut output = match manager_fallback.result {
        Ok(output) => {
            record_agent_observation(
                "fanout-manager",
                Some(session.id.as_str()),
                "manager",
                &manager_ran,
                &manager_args.cwd,
                &synthesis_prompt,
                &output,
                manager_started.elapsed(),
            );
            output
        }
        Err(err) => {
            record_agent_error(
                "fanout-manager",
                Some(session.id.as_str()),
                "manager",
                &manager_ran,
                &manager_args.cwd,
                &synthesis_prompt,
                &err,
                manager_started.elapsed(),
            );
            session.append_event(
                EventKind::ManagerFailed,
                serde_json::json!({
                    "agent": describe_spec(&manager_ran),
                    "error": err
                }),
            )?;
            write_text_atomic(std::path::Path::new(&tracking.stdout_path), "")?;
            write_text_atomic(std::path::Path::new(&tracking.stderr_path), &err)?;
            write_text_atomic(std::path::Path::new(&tracking.result_path), "")?;
            tracking.status = JobStatus::Failed;
            tracking.completed_at_ms = Some(now_ms());
            tracking.exit_code = Some(1);
            job_repo.save(&tracking).map_err(|e| e.to_string())?;
            return Err(err);
        }
    };
    output.stdout = capped_manager_output(&output.stdout);
    let code = output_status_code(&output);
    let job_status = if failed {
        JobStatus::Failed
    } else {
        output_record_status(&output, code)
    };
    let result = build_swarm_result_artifact(&worker_results, &output);
    let final_code = if failed { 1 } else { code };
    write_text_atomic(std::path::Path::new(&tracking.stdout_path), &result)?;
    write_text_atomic(std::path::Path::new(&tracking.stderr_path), &output.stderr)?;
    write_text_atomic(std::path::Path::new(&tracking.result_path), &result)?;
    tracking.status = job_status.clone();
    tracking.completed_at_ms = Some(now_ms());
    tracking.exit_code = Some(final_code);
    job_repo.save(&tracking).map_err(|e| e.to_string())?;
    write_text_atomic(&session.summary_path, output.stdout.trim())?;
    let transcript = build_swarm_transcript(&args.prompt, &worker_results, &output);
    write_text_atomic(&session.transcript_path, transcript)?;
    session.append_event(
        EventKind::ManagerCompleted,
        serde_json::json!({
            "agent": describe_spec(&args.manager),
            "exit_code": code,
            "timed_out": output.timed_out,
            "text": output.stdout.trim(),
            "stderr": output.stderr.trim()
        }),
    )?;
    session.append_layer_report(
        "manager",
        "manager",
        &describe_spec(&args.manager),
        None,
        job_status.as_str(),
        output.stdout.trim(),
    )?;
    session.append_event(
        EventKind::SessionCompleted,
        serde_json::json!({
            "failed": failed,
            "summary_path": session.summary_path.display().to_string(),
            "transcript_path": session.transcript_path.display().to_string(),
            "events_path": session.events_path.display().to_string()
        }),
    )?;
    print_partner_output(
        swarm_kernel::resolver::resolve_agent(manager_ran.agent)?,
        &manager_args,
        output,
    )?;
    println!("\nSession: {}", session.id);
    println!("Events: {}", session.events_path.display());
    println!("Transcript: {}", session.transcript_path.display());

    Ok(if failed { 1 } else { 0 })
}

pub fn run_discussion(args: DiscussArgs) -> Result<i32, String> {
    if args.participants.is_empty() {
        return Err("Error: discuss requires at least one available participant".to_string());
    }
    if args.rounds == 0 {
        return Err("Error: --rounds must be at least 1".to_string());
    }

    // Load config early so docs_enabled can be emitted in the SessionStarted event.
    let config = load_config();
    // Resolve docs: CLI flag > config.settings.docs_default > false.
    let docs_enabled = resolve_docs(args.docs, config.settings.docs_default);

    let session = DiscussionSession::create(&args)?;
    session.write_metadata(&args)?;
    let job_repo = FileJobRepo::new(job_store_dir().map_err(|e| e.to_string())?);
    let mut tracking = job_repo
        .create(JobSpec {
            agent: JobAgent::Swarm,
            model: Some(describe_spec(&args.manager)),
            mode: JobMode::Discussion,
            cwd: args.cwd.clone(),
            prompt_preview: prompt_preview(&args.prompt),
            prompt_text: args.prompt.clone(),
            timeout_secs: args.timeout_secs,
            allow_recursive_codex: false,
        })
        .map_err(|e| e.to_string())?;
    session.append_event(
        EventKind::SessionStarted,
        serde_json::json!({
            "prompt": &args.prompt,
            "cwd": args.cwd.display().to_string(),
            "rounds": args.rounds,
            "participants": args.participants.iter().map(|worker| {
                serde_json::json!({
                    "role": &worker.role,
                    "agent": describe_spec(&worker.spec),
                    "profile": profiles::profile_id_for_role(&worker.role)
                })
            }).collect::<Vec<_>>(),
            "manager": describe_spec(&args.manager),
            "docs": docs_enabled,
            "docs_agent": describe_spec(&args.docs_agent),
            "profile_helpers": args.profile_helpers
        }),
    )?;
    if let Err(_err) = run_session_preflight(
        &session,
        &BackendRegistry::from_config(&config),
        &args.manager,
        &args.participants,
    ) {
        session.append_event(
            EventKind::SessionCompleted,
            serde_json::json!({"failed": true}),
        )?;
        write_text_atomic(std::path::Path::new(&tracking.stdout_path), "")?;
        write_text_atomic(
            std::path::Path::new(&tracking.stderr_path),
            "preflight failed",
        )?;
        write_text_atomic(std::path::Path::new(&tracking.result_path), "")?;
        tracking.status = JobStatus::Failed;
        tracking.completed_at_ms = Some(now_ms());
        tracking.exit_code = Some(1);
        job_repo.save(&tracking).map_err(|e| e.to_string())?;
        return Ok(1);
    }

    println!("Starting discussion session {}", session.id);
    println!("  events:     {}", session.events_path.display());
    println!("  transcript: {}", session.transcript_path.display());
    println!("  summary:    {}", session.summary_path.display());
    io::stdout().flush().ok();

    let mut transcript = format!(
        "# Agent Swarm Discussion {}\n\nTask:\n{}\n\n",
        session.id, &args.prompt
    );
    write_text_atomic(&session.transcript_path, &transcript)?;
    let mut discussion_digest = String::from("No prior turns.");
    write_text_atomic(&session.digest_path, &discussion_digest)?;

    let mut turns = Vec::new();
    let mut failed = false;
    for round in 1..=args.rounds {
        let discussion_context = discussion_digest.clone();
        let mut handles = Vec::new();
        for (index, participant) in args.participants.clone().into_iter().enumerate() {
            let prompt = args.prompt.clone();
            let cwd = args.cwd.clone();
            let session = session.clone();
            let timeout_secs = participant.timeout_secs.unwrap_or(args.timeout_secs);
            let discussion_context = discussion_context.clone();
            let profile_helpers = args.profile_helpers;
            let config = config.clone();
            handles.push(thread::spawn(move || {
                let profile = profiles::profile_for_role(&participant.role);
                let _ = session.append_event(
                    EventKind::ProfileAssigned,
                    serde_json::json!({
                        "round": round,
                        "role": &participant.role,
                        "agent": describe_spec(&participant.spec),
                        "profile": profile.id,
                        "profile_title": profile.title,
                        "automation_hooks": profile.automation_hooks,
                        "deterministic_checks": profile.deterministic_checks,
                        "helper_count": if profile_helpers { profile.helpers.len() } else { 0 }
                    }),
                );
                let _ = session.append_event(
                    EventKind::AgentMessage,
                    serde_json::json!({
                        "round": round,
                        "from": "manager",
                        "to": &participant.role,
                        "direction": "outbound",
                        "agent": describe_spec(&participant.spec),
                        "text": "Context dispatched for profile-scoped turn."
                    }),
                );
                let helper_context = if profile_helpers {
                    run_profile_helpers(
                        &session,
                        &prompt,
                        &discussion_context,
                        round,
                        &participant,
                        &cwd,
                        timeout_secs,
                        &config,
                    )
                    .unwrap_or_else(|err| format!("Profile helper dispatch failed: {err}"))
                } else {
                    String::new()
                };
                let turn_prompt = build_discussion_turn_prompt(
                    &prompt,
                    &discussion_context,
                    round,
                    &participant,
                    &helper_context,
                );
                let _ = session.append_event(
                    EventKind::TurnStarted,
                    serde_json::json!({
                        "round": round,
                        "role": &participant.role,
                        "agent": describe_spec(&participant.spec)
                    }),
                );
                thread::sleep(Duration::from_millis((index as u64 % 6) * 90));
                let call_args = Args {
                    prompt: turn_prompt.clone(),
                    cwd,
                    timeout_secs,
                    quiet: true,
                    agent: participant.spec.agent,
                    agent_custom: participant.spec.custom.clone(),
                    model: participant.spec.model.clone(),
                    persona: None,
                    background: false,
                    allow_bypass_permissions: false,
                };
                let started = Instant::now();
                let agent_label = describe_spec(&participant.spec);
                let role_label = participant.role.clone();
                let heartbeat_done = Arc::new(AtomicBool::new(false));
                let heartbeat_flag = heartbeat_done.clone();
                let heartbeat_session = session.clone();
                let heartbeat_agent = agent_label.clone();
                let heartbeat_role = role_label.clone();
                let heartbeat = thread::spawn(move || {
                    let started = Instant::now();
                    while !heartbeat_flag.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_secs(6));
                        if heartbeat_flag.load(Ordering::Relaxed) {
                            break;
                        }
                        let _ = heartbeat_session.append_event_with_context(
                            EventKind::TurnHeartbeat,
                            serde_json::json!({
                                "round": round,
                                "role": &heartbeat_role,
                                "agent": &heartbeat_agent,
                                "elapsed_ms": started.elapsed().as_millis()
                            }),
                            Some("manager"),
                            &heartbeat_agent,
                            &heartbeat_role,
                            "turn",
                        );
                    }
                });
                let mut emit_chunk = |stream: &str, text: &str| {
                    let _ = session.append_event_with_context(
                        EventKind::TurnChunk,
                        serde_json::json!({
                            "round": round,
                            "role": role_label,
                            "agent": agent_label,
                            "stream": stream,
                            "text": preview_for_event(text, 1200)
                        }),
                        Some("manager"),
                        &agent_label,
                        &role_label,
                        "turn",
                    );
                };
                let chain = build_fallback_chain(&participant.role, &participant.spec, &config);
                let fallback = execute_with_fallback_chunks(
                    &BackendRegistry::from_config(&config),
                    &chain,
                    &call_args,
                    &turn_prompt,
                    &participant.role,
                    &config.reliability,
                    &mut emit_chunk,
                );
                heartbeat_done.store(true, Ordering::Relaxed);
                let _ = heartbeat.join();
                emit_reliability_events(&session, &participant.role, &participant.spec, &fallback);
                let output = fallback.result;
                match &output {
                    Ok(output) => {
                        record_agent_observation(
                            "discussion-turn",
                            Some(session.id.as_str()),
                            &participant.role,
                            &participant.spec,
                            &call_args.cwd,
                            &turn_prompt,
                            output,
                            started.elapsed(),
                        );
                        let code = output_status_code(output);
                        let _ = session.append_event(
                            EventKind::TurnCompleted,
                            serde_json::json!({
                                "round": round,
                                "role": &participant.role,
                                "agent": describe_spec(&participant.spec),
                                "exit_code": code,
                                "timed_out": output.timed_out,
                                "text": preview_for_event(output.stdout.trim(), 800),
                                "stderr": preview_for_event(output.stderr.trim(), 800)
                            }),
                        );
                        let _ = session.append_event(
                            EventKind::AgentMessage,
                            serde_json::json!({
                                "round": round,
                                "from": &participant.role,
                                "to": "manager",
                                "direction": "inbound",
                                "agent": describe_spec(&participant.spec),
                                "text": preview_for_event(output.stdout.trim(), 360)
                            }),
                        );
                        let _ = session.append_layer_report(
                            "participant",
                            &participant.role,
                            &describe_spec(&participant.spec),
                            Some("manager"),
                            if output.timed_out || code != 0 {
                                "needs_check"
                            } else {
                                "completed"
                            },
                            output.stdout.trim(),
                        );
                    }
                    Err(err) => {
                        record_agent_error(
                            "discussion-turn",
                            Some(session.id.as_str()),
                            &participant.role,
                            &participant.spec,
                            &call_args.cwd,
                            &turn_prompt,
                            err,
                            started.elapsed(),
                        );
                        let _ = session.append_event(
                            EventKind::TurnFailed,
                            classified_agent_error_payload(round, &participant, err),
                        );
                        let _ = session.append_layer_report(
                            "participant",
                            &participant.role,
                            &describe_spec(&participant.spec),
                            Some("manager"),
                            "failed",
                            err,
                        );
                    }
                }
                (participant, output)
            }));
        }

        for handle in handles {
            let (participant, output) = match handle.join() {
                Ok(result) => result,
                Err(_) => {
                    failed = true;
                    session.append_event(
                        EventKind::TurnFailed,
                        classified_error_payload("thread-panic", "participant thread panicked"),
                    )?;
                    continue;
                }
            };
            match output {
                Ok(output) => {
                    let code = output.exit_status.unwrap_or(1);
                    if output.timed_out || code != 0 {
                        failed = true;
                    }
                    let text = output.stdout.trim().to_string();
                    turns.push(DiscussionTurn {
                        round,
                        role: participant.role.clone(),
                        spec: participant.spec.clone(),
                        code,
                        timed_out: output.timed_out,
                        text: text.clone(),
                        stderr: output.stderr.trim().to_string(),
                    });
                    transcript.push_str(&format!(
                        "## Round {round} - {} ({})\n\n{}\n\n",
                        participant.role,
                        describe_spec(&participant.spec),
                        if text.is_empty() {
                            "(no output)"
                        } else {
                            &text
                        }
                    ));
                    write_text_atomic(&session.transcript_path, &transcript)?;
                    if output.timed_out || code != 0 {
                        session.append_event(
                            EventKind::TurnHealthCheck,
                            classified_agent_error_payload(
                                round,
                                &participant,
                                if output.timed_out {
                                    "agent timed out"
                                } else {
                                    output.stderr.trim()
                                },
                            ),
                        )?;
                    }
                }
                Err(err) => {
                    failed = true;
                    turns.push(DiscussionTurn {
                        round,
                        role: participant.role.clone(),
                        spec: participant.spec.clone(),
                        code: 1,
                        timed_out: false,
                        text: String::new(),
                        stderr: err.clone(),
                    });
                }
            }
        }
        write_text_atomic(&session.transcript_path, &transcript)?;
        discussion_digest = build_discussion_digest(&args.prompt, &turns, 14_000);
        write_text_atomic(&session.digest_path, &discussion_digest)?;
        session.append_event(
            EventKind::DiscussionDigestUpdated,
            serde_json::json!({
                "round": round,
                "path": session.digest_path.display().to_string(),
                "bytes": discussion_digest.len(),
                "text": preview_for_event(&discussion_digest, 1400)
            }),
        )?;
    }

    session.append_event(
        EventKind::ManagerStarted,
        serde_json::json!({
            "agent": describe_spec(&args.manager)
        }),
    )?;
    let manager_prompt = build_discussion_manager_prompt(&args.prompt, &discussion_digest, &turns);
    let manager_args = Args {
        prompt: manager_prompt.clone(),
        cwd: args.cwd.clone(),
        timeout_secs: args.timeout_secs,
        quiet: true,
        agent: args.manager.agent,
        agent_custom: args.manager.custom.clone(),
        model: args.manager.model.clone(),
        persona: None,
        background: false,
        allow_bypass_permissions: false,
    };
    let manager_chain = build_fallback_chain("manager", &args.manager, &config);
    let manager_started = Instant::now();
    let manager_fallback = execute_with_fallback(
        &BackendRegistry::from_config(&config),
        &manager_chain,
        &manager_args,
        &manager_prompt,
        "manager",
        &config.reliability,
    );
    emit_reliability_events(&session, "manager", &args.manager, &manager_fallback);
    let mut manager_output = match manager_fallback.result {
        Ok(output) => output,
        Err(err) => {
            record_agent_error(
                "discussion-manager",
                Some(session.id.as_str()),
                "manager",
                &args.manager,
                &manager_args.cwd,
                &manager_prompt,
                &err,
                manager_started.elapsed(),
            );
            session.append_event(
                EventKind::ManagerFailed,
                serde_json::json!({
                    "agent": describe_spec(&args.manager),
                    "error": err
                }),
            )?;
            write_text_atomic(std::path::Path::new(&tracking.stdout_path), "")?;
            write_text_atomic(
                std::path::Path::new(&tracking.stderr_path),
                "manager failed",
            )?;
            write_text_atomic(std::path::Path::new(&tracking.result_path), "")?;
            tracking.status = JobStatus::Failed;
            tracking.completed_at_ms = Some(now_ms());
            tracking.exit_code = Some(1);
            job_repo.save(&tracking).map_err(|e| e.to_string())?;
            return Ok(1);
        }
    };
    manager_output.stdout = capped_manager_output(&manager_output.stdout);
    record_agent_observation(
        "discussion-manager",
        Some(session.id.as_str()),
        "manager",
        &args.manager,
        &manager_args.cwd,
        &manager_prompt,
        &manager_output,
        manager_started.elapsed(),
    );
    let manager_code = manager_output.exit_status.unwrap_or(1);
    let mut summary = manager_output.stdout.trim().to_string();
    if manager_output.timed_out || manager_code != 0 {
        failed = true;
    }
    session.append_event(
        EventKind::ManagerCompleted,
        serde_json::json!({
            "agent": describe_spec(&args.manager),
            "exit_code": manager_code,
            "timed_out": manager_output.timed_out,
            "text": &summary,
            "stderr": manager_output.stderr.trim()
        }),
    )?;
    session.append_layer_report(
        "manager",
        "manager",
        &describe_spec(&args.manager),
        None,
        if manager_output.timed_out || manager_code != 0 {
            "needs_check"
        } else {
            "completed"
        },
        &summary,
    )?;

    if docs_enabled {
        session.append_event(
            EventKind::DocsStarted,
            serde_json::json!({
                "agent": describe_spec(&args.docs_agent)
            }),
        )?;
        let docs_prompt = build_docs_prompt(&args.prompt, &transcript, &summary);
        let docs_args = Args {
            prompt: docs_prompt.clone(),
            cwd: args.cwd.clone(),
            timeout_secs: args.timeout_secs,
            quiet: true,
            agent: args.docs_agent.agent,
            agent_custom: args.docs_agent.custom.clone(),
            model: args.docs_agent.model.clone(),
            persona: None,
            background: false,
            allow_bypass_permissions: false,
        };
        let docs_chain = build_fallback_chain("api-docs", &args.docs_agent, &config);
        let docs_started = Instant::now();
        let docs_fallback = execute_with_fallback(
            &BackendRegistry::from_config(&config),
            &docs_chain,
            &docs_args,
            &docs_prompt,
            "api-docs",
            &config.reliability,
        );
        emit_reliability_events(&session, "api-docs", &args.docs_agent, &docs_fallback);
        match docs_fallback.result {
            Ok(output) => {
                record_agent_observation(
                    "discussion-docs",
                    Some(session.id.as_str()),
                    "api-docs",
                    &args.docs_agent,
                    &docs_args.cwd,
                    &docs_prompt,
                    &output,
                    docs_started.elapsed(),
                );
                let code = output.exit_status.unwrap_or(1);
                if output.timed_out || code != 0 {
                    failed = true;
                }
                let docs_text = output.stdout.trim();
                write_text_atomic(&session.docs_path, docs_text)?;
                if !docs_text.is_empty() {
                    summary.push_str("\n\n## API Documentation Follow-Up\n\n");
                    summary.push_str(docs_text);
                }
                session.append_event(
                    EventKind::DocsCompleted,
                    serde_json::json!({
                        "agent": describe_spec(&args.docs_agent),
                        "exit_code": code,
                        "timed_out": output.timed_out,
                        "path": session.docs_path.display().to_string(),
                        "text": docs_text,
                        "stderr": output.stderr.trim()
                    }),
                )?;
            }
            Err(err) => {
                record_agent_error(
                    "discussion-docs",
                    Some(session.id.as_str()),
                    "api-docs",
                    &args.docs_agent,
                    &docs_args.cwd,
                    &docs_prompt,
                    &err,
                    docs_started.elapsed(),
                );
                failed = true;
                session.append_event(
                    EventKind::DocsFailed,
                    serde_json::json!({
                        "agent": describe_spec(&args.docs_agent),
                        "error": err
                    }),
                )?;
            }
        }
    }

    write_text_atomic(&session.summary_path, &summary)?;
    session.append_event(
        EventKind::SessionCompleted,
        serde_json::json!({
            "failed": failed,
            "summary_path": session.summary_path.display().to_string(),
            "transcript_path": session.transcript_path.display().to_string(),
            "events_path": session.events_path.display().to_string()
        }),
    )?;

    if !summary.trim().is_empty() {
        println!("\n{summary}");
    }
    println!("\nSession: {}", session.id);
    println!("Events: {}", session.events_path.display());
    println!("Transcript: {}", session.transcript_path.display());
    let job_status = if failed {
        JobStatus::Failed
    } else {
        JobStatus::Completed
    };
    let exit_code = if failed { 1 } else { 0 };
    write_text_atomic(std::path::Path::new(&tracking.stdout_path), &summary)?;
    write_text_atomic(
        std::path::Path::new(&tracking.stderr_path),
        &manager_output.stderr,
    )?;
    write_text_atomic(std::path::Path::new(&tracking.result_path), &summary)?;
    tracking.status = job_status;
    tracking.completed_at_ms = Some(now_ms());
    tracking.exit_code = Some(exit_code);
    job_repo.save(&tracking).map_err(|e| e.to_string())?;
    Ok(exit_code)
}

// Unlike `cmd_presets`, this command expands a selected preset into swarm or
// discussion orchestration, so it stays with the orchestration entry points.
pub fn cmd_preset(raw: &[String]) -> Result<i32, String> {
    let preset_id = PresetId::from(
        raw.first()
            .ok_or_else(|| "Error: preset requires a preset id".to_string())?
            .as_str(),
    );
    let mut passthrough = Vec::new();
    let mut prompt_parts = Vec::new();
    let mut helper_flag_seen = false;
    let mut iter = raw.iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--cwd" | "--timeout" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("Error: {arg} requires a value"))?;
                passthrough.push(arg.clone());
                passthrough.push(value.clone());
            }
            "--helpers" | "--profile-helpers" | "--no-helpers" => {
                helper_flag_seen = true;
                passthrough.push(arg.clone());
            }
            "-h" | "--help" => {
                print_help(load_default_timeout());
                return Ok(0);
            }
            other => prompt_parts.push(other.to_string()),
        }
    }
    let prompt = prompt_parts.join(" ");
    if prompt.trim().is_empty() {
        return Err("Error: preset requires a prompt".to_string());
    }
    if !helper_flag_seen {
        passthrough.push("--helpers".to_string());
    }

    match preset_id.as_str() {
        "architecture-council" => {
            let mut args = vec![
                "--manager".to_string(),
                "claude:sonnet".to_string(),
                "--participant".to_string(),
                "systems=gemini".to_string(),
                "--participant".to_string(),
                "tradeoffs=claude:sonnet".to_string(),
                "--participant".to_string(),
                "migration=gemini".to_string(),
                "--rounds".to_string(),
                "2".to_string(),
            ];
            args.extend(passthrough);
            args.push(prompt);
            run_discussion(parse_discuss_args(args)?)
        }
        "codebase-audit" => {
            let mut args = vec![
                "--focus".to_string(),
                "all".to_string(),
                "--manager".to_string(),
                "claude:sonnet".to_string(),
                "--participant".to_string(),
                "architecture=gemini".to_string(),
                "--participant".to_string(),
                "simplify=claude:sonnet".to_string(),
                "--participant".to_string(),
                "hardening=claude:sonnet".to_string(),
                "--rounds".to_string(),
                "1".to_string(),
                "--docs".to_string(),
            ];
            args.extend(passthrough);
            args.push(prompt);
            run_discussion(parse_audit_args(args)?)
        }
        "ui-polish" => {
            let mut args = vec![
                "--focus".to_string(),
                "implementation".to_string(),
                "--manager".to_string(),
                "claude:sonnet".to_string(),
                "--participant".to_string(),
                "product-design=gemini".to_string(),
                "--participant".to_string(),
                "motion-accessibility=claude:sonnet".to_string(),
                "--participant".to_string(),
                "component-architecture=gemini".to_string(),
                "--rounds".to_string(),
                "1".to_string(),
            ];
            args.extend(passthrough);
            args.push(prompt);
            run_discussion(parse_design_args(args)?)
        }
        "regression-hunt" => {
            let mut args = vec![
                "--manager".to_string(),
                "claude:sonnet".to_string(),
                "--worker".to_string(),
                "repro=gemini".to_string(),
                "--worker".to_string(),
                "root-cause=claude:sonnet".to_string(),
                "--worker".to_string(),
                "tests=gemini".to_string(),
            ];
            args.extend(passthrough);
            args.push(prompt);
            run_swarm(parse_swarm_args(args)?)
        }
        "api-docs-followup" => {
            let mut args = vec![
                "--manager".to_string(),
                "claude:sonnet".to_string(),
                "--participant".to_string(),
                "api-docs=claude:sonnet".to_string(),
                "--participant".to_string(),
                "examples=gemini".to_string(),
                "--rounds".to_string(),
                "1".to_string(),
            ];
            args.extend(passthrough);
            args.push(prompt);
            run_discussion(parse_discuss_args(args)?)
        }
        other => Err(format!(
            "Error: unknown preset `{other}`. Run `agent-swarm presets` to list available presets."
        )),
    }
}

// Helper dispatch threads the full per-round discussion context (session, task,
// transcript, round, participant, cwd, timeout, config); bundling into a struct
// would be churn for a single private call site.
#[allow(clippy::too_many_arguments)]
fn run_profile_helpers(
    session: &DiscussionSession,
    task: &str,
    transcript: &str,
    round: u32,
    participant: &WorkerSpec,
    cwd: &Path,
    timeout_secs: u64,
    config: &SwarmConfig,
) -> Result<String, String> {
    let helpers = profiles::helpers_for_role(&participant.role);
    if helpers.is_empty() {
        return Ok(String::new());
    }

    let mut context = String::new();
    for helper in helpers.iter().take(2) {
        let spec = parse_agent_spec_struct(helper.agent)?;
        let helper_prompt =
            build_profile_helper_prompt(task, transcript, round, participant, helper);
        let helper_timeout = timeout_secs.clamp(30, 180);
        session.append_event(
            EventKind::HelperStarted,
            serde_json::json!({
                "round": round,
                "role": helper.role,
                "agent": describe_spec(&spec),
                "parent_role": &participant.role,
                "purpose": helper.purpose
            }),
        )?;
        session.append_event(
            EventKind::AgentMessage,
            serde_json::json!({
                "round": round,
                "from": &participant.role,
                "to": helper.role,
                "direction": "outbound",
                "agent": describe_spec(&spec),
                "text": helper.purpose
            }),
        )?;
        let call_args = Args {
            prompt: helper_prompt.clone(),
            cwd: cwd.to_path_buf(),
            timeout_secs: helper_timeout,
            quiet: true,
            agent: spec.agent,
            agent_custom: spec.custom.clone(),
            model: spec.model.clone(),
            persona: None,
            background: false,
            allow_bypass_permissions: false,
        };
        let started = Instant::now();
        let helper_chain = build_fallback_chain(helper.role, &spec, config);
        let helper_fallback = execute_with_fallback(
            &BackendRegistry::from_config(config),
            &helper_chain,
            &call_args,
            &helper_prompt,
            helper.role,
            &config.reliability,
        );
        emit_reliability_events(session, helper.role, &spec, &helper_fallback);
        match helper_fallback.result {
            Ok(output) => {
                record_agent_observation(
                    "profile-helper",
                    Some(session.id.as_str()),
                    helper.role,
                    &spec,
                    &call_args.cwd,
                    &helper_prompt,
                    &output,
                    started.elapsed(),
                );
                session.append_event(
                    EventKind::HelperCompleted,
                    serde_json::json!({
                        "round": round,
                        "role": helper.role,
                        "agent": describe_spec(&spec),
                        "parent_role": &participant.role,
                        "exit_code": output_status_code(&output),
                        "timed_out": output.timed_out,
                        "text": output.stdout.trim(),
                        "stderr": output.stderr.trim()
                    }),
                )?;
                session.append_layer_report(
                    "helper",
                    helper.role,
                    &describe_spec(&spec),
                    Some(&participant.role),
                    if output.timed_out || output_status_code(&output) != 0 {
                        "needs_check"
                    } else {
                        "completed"
                    },
                    output.stdout.trim(),
                )?;
                session.append_event(
                    EventKind::AgentMessage,
                    serde_json::json!({
                        "round": round,
                        "from": helper.role,
                        "to": &participant.role,
                        "direction": "inbound",
                        "agent": describe_spec(&spec),
                        "text": preview_for_event(output.stdout.trim(), 320)
                    }),
                )?;
                if !output.stdout.trim().is_empty() {
                    context.push_str(&format!(
                        "\n### Helper: {} ({})\n{}\n",
                        helper.role,
                        describe_spec(&spec),
                        output.stdout.trim()
                    ));
                }
            }
            Err(err) => {
                record_agent_error(
                    "profile-helper",
                    Some(session.id.as_str()),
                    helper.role,
                    &spec,
                    &call_args.cwd,
                    &helper_prompt,
                    &err,
                    started.elapsed(),
                );
                session.append_event(
                    EventKind::HelperFailed,
                    serde_json::json!({
                        "round": round,
                        "role": helper.role,
                        "agent": describe_spec(&spec),
                        "parent_role": &participant.role,
                        "error": err
                    }),
                )?;
                session.append_layer_report(
                    "helper",
                    helper.role,
                    &describe_spec(&spec),
                    Some(&participant.role),
                    "failed",
                    &err,
                )?;
            }
        }
    }
    Ok(context)
}
