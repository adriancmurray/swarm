//! Prompt, transcript, and artifact synthesis helpers.

use swarm_kernel::agent::describe_spec;
use swarm_kernel::args::WorkerSpec;
use swarm_kernel::profiles;

use crate::session::DiscussionTurn;
use swarm_kernel::backend_abi::RunOutcome;

const MANAGER_WORKER_STDOUT_BYTES: usize = 700;
const MANAGER_UNVERIFIED_WORKER_STDOUT_BYTES: usize = 450;
const MANAGER_WORKER_STDERR_BYTES: usize = 500;
const MANAGER_COMPACT_OUTPUT_BYTES: usize = 3_000;
const WORKER_COMPACT_OUTPUT_BYTES: usize = 2_400;

pub use swarm_kernel::prompts::COMPACT_HANDOFF_CONTRACT;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerEvidenceGate {
    pub verified: bool,
    pub has_blockers: bool,
    pub citation_count: usize,
    pub evidence_gap_count: usize,
    pub stdout_bytes: usize,
    pub flags: Vec<String>,
}

impl WorkerEvidenceGate {
    fn manager_status(&self) -> &'static str {
        if self.verified {
            "VERIFIED"
        } else {
            "UNVERIFIED"
        }
    }
}

pub fn assess_worker_output(
    exit_code: i32,
    timed_out: bool,
    stdout: &str,
    stderr: &str,
) -> WorkerEvidenceGate {
    let trimmed = stdout.trim();
    let lower = trimmed.to_ascii_lowercase();
    let citation_count = count_citations(trimmed);
    let evidence_gap_count =
        count_evidence_gaps(trimmed).saturating_add(count_evidence_gaps(stderr));
    let stdout_bytes = trimmed.len();
    let mut flags = Vec::new();
    if timed_out {
        flags.push("TIMED_OUT".to_string());
    }
    if exit_code != 0 {
        flags.push("NONZERO_EXIT".to_string());
    }
    if trimmed.is_empty() {
        flags.push("EMPTY_OUTPUT".to_string());
    }
    if citation_count == 0 {
        flags.push("NO_CITATIONS".to_string());
    }
    if evidence_gap_count > 0 {
        flags.push("EVIDENCE_GAP".to_string());
    }
    if stdout_bytes > WORKER_COMPACT_OUTPUT_BYTES {
        flags.push("OVER_COMPACT_CAP".to_string());
    }
    if missing_packet_sections(&lower) {
        flags.push("MISSING_PACKET_SECTIONS".to_string());
    }
    let has_blockers =
        has_blocker_signal(trimmed) || evidence_gap_count > 0 || timed_out || exit_code != 0;
    if has_blockers {
        flags.push("BLOCKER_SIGNAL".to_string());
    }
    let verified = flags.is_empty();
    WorkerEvidenceGate {
        verified,
        has_blockers,
        citation_count,
        evidence_gap_count,
        stdout_bytes,
        flags,
    }
}

fn count_citations(text: &str) -> usize {
    text.split_whitespace()
        .filter(|token| {
            [
                "file://", "](/", "#L", ".rs:", ".dart:", ".md:", ".toml:", ".json:", "/Users/",
            ]
            .iter()
            .any(|needle| token.contains(needle))
        })
        .count()
}

fn count_evidence_gaps(text: &str) -> usize {
    let lower = text.to_ascii_lowercase();
    [
        "needs_evidence",
        "missing evidence",
        "uncited",
        "could not verify",
    ]
    .iter()
    .map(|needle| lower.matches(needle).count())
    .sum()
}

fn missing_packet_sections(lower: &str) -> bool {
    ["findings", "risks", "steps", "blockers", "tests"]
        .iter()
        .any(|section| !lower.contains(section))
}

fn has_blocker_signal(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("needs_evidence") {
        return true;
    }
    let Some(idx) = lower.find("blockers") else {
        return false;
    };
    let section = lower[idx..].lines().take(4).collect::<Vec<_>>().join("\n");
    !(section.contains("none") || section.contains("0 bullets") || section.contains("no blocker"))
}

// preview_for_event lives in swarm-kernel::format (P5-S2.5).
// Re-exported here so orchestration.rs, monitor_runtime.rs, and agent-swarm
// shim callers can reference it via `crate::synthesis::preview_for_event`.
pub use swarm_kernel::format::preview_for_event;

/// Renders a pre-gathered context JSON value (from `context_gather_json`) into
/// a bounded plain-text block suitable for injection into agent prompts.
///
/// The block is headed by `--- local context (auto-gathered, cwd: <cwd>) ---`
/// and closed by `--- end context ---`. Each symbol entry is rendered as a
/// single `<path>: <excerpt>` line (excerpt is pre-truncated by the gatherer).
/// Returns `None` if `context` contains no symbols.
pub fn render_context_block(context: &serde_json::Value) -> Option<String> {
    let cwd = context
        .get("cwd")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let symbols = context.get("symbols").and_then(|value| value.as_array())?;
    if symbols.is_empty() {
        return None;
    }
    let mut block = format!("--- local context (auto-gathered, cwd: {cwd}) ---\n");
    for symbol in symbols {
        let path = symbol
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or("?");
        let excerpt = symbol
            .get("excerpt")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        if excerpt.is_empty() {
            block.push_str(&format!("{path}\n"));
        } else {
            block.push_str(&format!("{path}: {excerpt}\n"));
        }
    }
    block.push_str("--- end context ---\n");
    Some(block)
}

/// Builds the per-worker prompt. When `context` is `Some`, the auto-gathered
/// context block is appended after the task text. When `None`, the output is
/// byte-identical to the pre-injection implementation (OFF path is the default).
pub fn build_worker_prompt(task: &str, role: &str, context: Option<&str>) -> String {
    let mut prompt = format!(
        "You are the `{role}` worker in a stacked agent swarm.\n\
         Analyze the task from your specialty only. Be concrete, concise, and actionable.\n\
         Do not edit files.\n\
         {COMPACT_HANDOFF_CONTRACT}\n\n\
         Task:\n{task}"
    );
    if let Some(ctx) = context {
        prompt.push_str("\n\n");
        prompt.push_str(ctx);
    }
    prompt
}

pub fn build_direct_persona_prompt(task: &str, persona: &str) -> Result<String, String> {
    let persona = persona.trim();
    if persona.is_empty()
        || matches!(
            normalize_persona(persona).as_str(),
            "none" | "off" | "disabled"
        )
    {
        return Ok(task.to_string());
    }

    match normalize_persona(persona).as_str() {
        "compact-manager" | "manager" | "metadirector" | "meta-director" => Ok(format!(
            "You are a compact metadirector for a multi-agent system.\n\
             Optimize for quality per token: synthesize, verify, and escalate instead of narrating.\n\
             Hard rules:\n\
             - Do not narrate tool use, file reads, or process steps.\n\
             - Use source citations for claims about files, commands, docs, or previous artifacts.\n\
             - First establish the current architecture from cited sources; never infer languages, modules, structs, classes, or files from memory.\n\
             - Reject stale or contradictory claims explicitly. If a cited path/symbol is missing, say so instead of inventing a replacement.\n\
             - If evidence is missing, write NEEDS_EVIDENCE with the missing anchor.\n\
             - Required sections: Verdict, Evidence, Rejected Claims, Risks, Next Slice, Tests.\n\
             - Evidence bullets must cite paths or commands. Rejected Claims should name hallucinations, stale assumptions, or uncited assertions.\n\
             - Maximum 650 words unless the user explicitly asks for long-form output.\n\n\
             Direct task:\n{task}"
        )),
        "gemini-large-context-manager"
        | "large-context-manager"
        | "wide-context-manager"
        | "gemini-manager" => Ok(format!(
            "You are a Gemini large-context metadirector for a multi-agent system.\n\
             Use the wide context window for intake and cross-file synthesis, but return a compact decision packet.\n\
             Hard rules:\n\
             - Do not narrate tool use, file reads, directory traversal, or process steps.\n\
             - Separate source-grounded facts from synthesis. Every architecture/file/API claim needs a citation.\n\
             - Build a Source Map first: 3-8 bullets naming inspected anchors and what each proves.\n\
             - Reject stale, contradictory, or uncited claims explicitly. If a path/symbol is missing, write NEEDS_EVIDENCE with the missing anchor.\n\
             - Prefer packetized context over transcript replay: cite packet ids, session artifacts, files, or commands instead of restating long history.\n\
             - Keep worker instructions bounded; delegate broad reading to scouts and final verification to a verifier when risk is nontrivial.\n\
             - Required sections, in order: Source Map, Verdict, Accepted Facts, Rejected Claims, Decision, Next Slice, Tests.\n\
             - Maximum 750 words unless the user explicitly asks for long-form output.\n\n\
             Direct task:\n{task}"
        )),
        "compact-worker" | "worker" | "handoff" => Ok(format!(
            "You are a compact worker in a stacked agent swarm.\n\
             Analyze only the requested specialty and return a handoff packet.\n\
             Do not edit files unless explicitly asked by the user.\n\
             {COMPACT_HANDOFF_CONTRACT}\n\n\
             Task:\n{task}"
        )),
        _ => {
            let profile = profiles::profile_by_id_or_role(persona).ok_or_else(|| {
                format!(
                    "Error: unknown direct persona `{persona}`. Use compact-manager, compact-worker, or an id/role from `agent-swarm profiles`."
                )
            })?;
            Ok(format!(
                "You are using the `{}` direct-agent profile.\n\
                 Purpose: {}.\n\
                 Profile default agent: {}.\n\
                 Deterministic checks: {}.\n\
                 Work compactly. Do not narrate file reads or tool plans. Cite inspected source paths. If evidence is missing, write NEEDS_EVIDENCE.\n\
                 Maximum 650 words unless the user explicitly asks for long-form output.\n\n\
                 Direct task:\n{task}",
                profile.id,
                profile.purpose,
                profile.default_agent,
                profile.deterministic_checks.join(", ")
            ))
        }
    }
}

fn normalize_persona(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

/// Builds the manager synthesis prompt. When `context` is `Some`, the
/// auto-gathered context block is appended after the task text (before worker
/// outputs). When `None`, the output is byte-identical to the pre-injection
/// implementation (OFF path is the default).
pub fn build_manager_prompt(
    task: &str,
    results: &[(WorkerSpec, i32, RunOutcome)],
    context: Option<&str>,
) -> String {
    let mut prompt = format!(
        "You are the manager agent synthesizing a stacked agent swarm.\n\
         Produce the final answer or implementation plan. Resolve disagreements, prioritize concrete next steps, and call out failed workers.\n\
         Treat timed-out, nonzero-exit, truncated, or citation-free worker claims as unverified.\n\
         Prefer verified source anchors over confident summaries. If evidence is missing, return NEEDS_EVIDENCE instead of filling gaps from memory.\n\
         Keep the synthesis compact: verdict, accepted facts, rejected/failed claims, next slice, tests, rollout risk.\n\
         Maximum 650 words. Do not narrate worker reading steps. Do not include uncited source claims.\n\n\
         Original task:\n{task}"
    );
    if let Some(ctx) = context {
        prompt.push_str("\n\n");
        prompt.push_str(ctx);
    }
    prompt.push_str("\n\nWorker outputs:\n");
    for (worker, code, output) in results {
        let gate = assess_worker_output(*code, output.timed_out, &output.stdout, &output.stderr);
        prompt.push_str(&format!(
            "\n--- worker: {} ({}) exit={} timed_out={} gate={} blockers={} citations={} evidence_gaps={} flags={} ---\n",
            worker.role,
            describe_spec(&worker.spec),
            code,
            output.timed_out,
            gate.manager_status(),
            gate.has_blockers,
            gate.citation_count,
            gate.evidence_gap_count,
            if gate.flags.is_empty() {
                "none".to_string()
            } else {
                gate.flags.join(",")
            }
        ));
        if !gate.verified {
            prompt.push_str(
                "Runtime evidence gate: treat this worker as unverified. Use only cited facts; do not synthesize uncited claims from this output.\n",
            );
        }
        if !output.stdout.trim().is_empty() {
            let cap = if gate.verified {
                MANAGER_WORKER_STDOUT_BYTES
            } else {
                MANAGER_UNVERIFIED_WORKER_STDOUT_BYTES
            };
            prompt.push_str(&capped_text(output.stdout.trim(), cap));
            prompt.push('\n');
        }
        if !output.stderr.trim().is_empty() {
            prompt.push_str("\nstderr:\n");
            prompt.push_str(&capped_text(
                output.stderr.trim(),
                MANAGER_WORKER_STDERR_BYTES,
            ));
            prompt.push('\n');
        }
    }
    prompt
}

pub fn build_swarm_result_artifact(
    results: &[(WorkerSpec, i32, RunOutcome)],
    manager: &RunOutcome,
) -> String {
    let mut artifact = String::from("# Agent Swarm Result\n\n## Manager Synthesis\n\n");
    if manager.stdout.trim().is_empty() {
        artifact.push_str("(no manager output)\n");
    } else {
        artifact.push_str(manager.stdout.trim());
        artifact.push('\n');
    }
    if !manager.stderr.trim().is_empty() {
        artifact.push_str("\n## Manager stderr\n\n");
        artifact.push_str(manager.stderr.trim());
        artifact.push('\n');
    }

    artifact.push_str("\n## Worker Outputs\n");
    for (worker, code, output) in results {
        artifact.push_str(&format!(
            "\n### {} ({})\n\nexit={} timed_out={}\n\n",
            worker.role,
            describe_spec(&worker.spec),
            code,
            output.timed_out
        ));
        if output.stdout.trim().is_empty() {
            artifact.push_str("(no stdout)\n");
        } else {
            artifact.push_str(output.stdout.trim());
            artifact.push('\n');
        }
        if !output.stderr.trim().is_empty() {
            artifact.push_str("\nstderr:\n");
            artifact.push_str(output.stderr.trim());
            artifact.push('\n');
        }
    }
    artifact
}

pub fn build_swarm_transcript(
    task: &str,
    results: &[(WorkerSpec, i32, RunOutcome)],
    manager: &RunOutcome,
) -> String {
    let mut transcript = format!("# Agent Swarm Fan-Out\n\nTask:\n{task}\n\n");
    transcript.push_str("## Workers\n");
    for (worker, code, output) in results {
        transcript.push_str(&format!(
            "\n### {} ({})\n\nexit={} timed_out={}\n\n",
            worker.role,
            describe_spec(&worker.spec),
            code,
            output.timed_out
        ));
        if output.stdout.trim().is_empty() {
            transcript.push_str("(no output)\n");
        } else {
            transcript.push_str(output.stdout.trim());
            transcript.push('\n');
        }
        if !output.stderr.trim().is_empty() {
            transcript.push_str("\nstderr:\n");
            transcript.push_str(output.stderr.trim());
            transcript.push('\n');
        }
    }
    transcript.push_str("\n## Manager\n\n");
    if manager.stdout.trim().is_empty() {
        transcript.push_str("(no manager output)\n");
    } else {
        transcript.push_str(manager.stdout.trim());
        transcript.push('\n');
    }
    if !manager.stderr.trim().is_empty() {
        transcript.push_str("\nstderr:\n");
        transcript.push_str(manager.stderr.trim());
        transcript.push('\n');
    }
    transcript
}

pub fn build_profile_helper_prompt(
    task: &str,
    discussion_context: &str,
    round: u32,
    participant: &WorkerSpec,
    helper: &profiles::ProfileHelper,
) -> String {
    format!(
        "You are a one-layer helper agent for the `{}` participant.\n\
         Helper role: `{}`.\n\
         Purpose: {}.\n\
         Round: {round}.\n\
         Do not edit files. Return only information that helps the parent participant do better work.\n\
         {COMPACT_HANDOFF_CONTRACT}\n\n\
         Original task:\n{task}\n\n\
         Bounded discussion context:\n{discussion_context}",
        participant.role, helper.role, helper.purpose
    )
}

pub fn build_discussion_turn_prompt(
    task: &str,
    discussion_context: &str,
    round: u32,
    participant: &WorkerSpec,
    helper_context: &str,
) -> String {
    let helper_section = if helper_context.trim().is_empty() {
        String::new()
    } else {
        format!("\n\nOne-layer helper context:\n{helper_context}")
    };
    format!(
        "You are the `{}` participant in a bounded multi-agent discussion.\n\
         Round: {round}.\n\
         Speak to the other agents, not just the user. Build on useful prior points, challenge weak claims, and stay concise.\n\
         Do not edit files.\n\
         {COMPACT_HANDOFF_CONTRACT}\n\n\
         Original task:\n{task}\n\n\
         Bounded discussion context:\n{discussion_context}{helper_section}",
        participant.role
    )
}

pub fn build_discussion_digest(task: &str, turns: &[DiscussionTurn], max_bytes: usize) -> String {
    let mut digest = format!(
        "# Rolling Discussion Digest\n\nOriginal task: {}\n\n",
        preview_for_event(task, 600)
    );
    if turns.is_empty() {
        digest.push_str("No participant turns have completed yet.\n");
        return digest;
    }
    let latest_round = turns.iter().map(|turn| turn.round).max().unwrap_or(1);
    digest.push_str("## Current State\n\n");
    for turn in turns.iter().filter(|turn| turn.round == latest_round) {
        digest.push_str(&format!(
            "- Round {} `{}` ({}) exit={} timed_out={}: {}\n",
            turn.round,
            turn.role,
            describe_spec(&turn.spec),
            turn.code,
            turn.timed_out,
            preview_for_event(&turn.text, 900)
        ));
        if !turn.stderr.trim().is_empty() {
            digest.push_str(&format!(
                "  stderr: {}\n",
                preview_for_event(&turn.stderr, 260)
            ));
        }
    }
    let prior = turns.iter().filter(|turn| turn.round < latest_round);
    if prior.clone().next().is_some() {
        digest.push_str("\n## Prior Rounds\n\n");
        for turn in prior {
            digest.push_str(&format!(
                "- R{} `{}`: {}\n",
                turn.round,
                turn.role,
                preview_for_event(&turn.text, 320)
            ));
        }
    }
    if digest.len() > max_bytes {
        let keep = max_bytes.saturating_sub(96);
        let truncated = digest.chars().take(keep).collect::<String>();
        format!("{truncated}\n\n[Digest truncated to {max_bytes} bytes.]\n")
    } else {
        digest
    }
}

pub fn build_discussion_manager_prompt(
    task: &str,
    discussion_digest: &str,
    turns: &[DiscussionTurn],
) -> String {
    let mut prompt = format!(
        "You are the manager synthesizing a bounded multi-agent discussion.\n\
         Produce a clear final recommendation. Preserve disagreements that matter, name risks, and identify the next implementation steps.\n\n\
         Maximum 650 words. Do not narrate participant reading steps. Do not include uncited source claims.\n\n\
         Original task:\n{task}\n\n\
         Rolling discussion digest:\n{discussion_digest}\n\n\
         Turn health and capped latest outputs:\n"
    );
    for turn in turns {
        let gate = assess_worker_output(turn.code, turn.timed_out, &turn.text, &turn.stderr);
        prompt.push_str(&format!(
            "\n--- round {}: {} ({}) exit={} timed_out={} gate={} blockers={} citations={} evidence_gaps={} flags={} ---\n",
            turn.round,
            turn.role,
            describe_spec(&turn.spec),
            turn.code,
            turn.timed_out,
            gate.manager_status(),
            gate.has_blockers,
            gate.citation_count,
            gate.evidence_gap_count,
            if gate.flags.is_empty() {
                "none".to_string()
            } else {
                gate.flags.join(",")
            }
        ));
        if !gate.verified {
            prompt.push_str(
                "Runtime evidence gate: treat this turn as unverified. Use only cited facts; do not synthesize uncited claims from this output.\n",
            );
        }
        if !turn.text.trim().is_empty() {
            let cap = if gate.verified {
                MANAGER_WORKER_STDOUT_BYTES
            } else {
                MANAGER_UNVERIFIED_WORKER_STDOUT_BYTES
            };
            prompt.push_str(&capped_text(turn.text.trim(), cap));
            prompt.push('\n');
        }
        if !turn.stderr.trim().is_empty() {
            prompt.push_str("\nstderr:\n");
            prompt.push_str(&capped_text(
                turn.stderr.trim(),
                MANAGER_WORKER_STDERR_BYTES,
            ));
            prompt.push('\n');
        }
    }
    prompt
}

fn capped_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let keep = max_bytes.saturating_sub(80);
    let truncated = text.chars().take(keep).collect::<String>();
    format!("{truncated}\n[output truncated to {max_bytes} bytes]")
}

pub fn capped_manager_output(text: &str) -> String {
    capped_text(text.trim(), MANAGER_COMPACT_OUTPUT_BYTES)
}

pub fn build_docs_prompt(task: &str, transcript: &str, synthesis: &str) -> String {
    format!(
        "You are an API documentation follow-up subagent.\n\
         Review the task, agent discussion, and final synthesis. Do not edit files.\n\
         Return documentation recommendations only: public APIs that need docs, missing examples, generated-doc targets, and concise doc comment drafts where obvious.\n\
         {COMPACT_HANDOFF_CONTRACT}\n\n\
         Original task:\n{task}\n\n\
         Discussion transcript:\n{transcript}\n\n\
         Manager synthesis:\n{synthesis}"
    )
}

/// Deterministic verification of the metadirector output contract.
/// Verifies that an output contains the required sections:
/// - Source Map
/// - Verdict or What Changed
/// - Accepted Facts or Verification
/// - Rejected Claims or Risks / Gaps
/// - Next Slice
/// - Tests
pub fn verify_metadirector_contract(text: &str) -> Result<(), Vec<String>> {
    let lower = text.to_ascii_lowercase();
    let mut missing = Vec::new();

    // Source Map
    if !lower.contains("source map") && !lower.contains("source-map") {
        missing.push("Source Map".to_string());
    }

    // Verdict or What Changed
    if !lower.contains("verdict")
        && !lower.contains("what changed")
        && !lower.contains("what-changed")
    {
        missing.push("Verdict or What Changed".to_string());
    }

    // Accepted Facts or Verification
    if !lower.contains("accepted facts")
        && !lower.contains("accepted-facts")
        && !lower.contains("verification")
    {
        missing.push("Accepted Facts or Verification".to_string());
    }

    // Rejected Claims or Risks / Gaps
    if !lower.contains("rejected claims")
        && !lower.contains("rejected-claims")
        && !lower.contains("risks / gaps")
        && !lower.contains("risks/gaps")
        && !lower.contains("risks")
        && !lower.contains("gaps")
    {
        missing.push("Rejected Claims or Risks / Gaps".to_string());
    }

    // Next Slice
    if !lower.contains("next slice") && !lower.contains("next-slice") {
        missing.push("Next Slice".to_string());
    }

    // Tests
    if !lower.contains("tests") {
        missing.push("Tests".to_string());
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarm_kernel::agent::{AgentChoice, AgentSpec};

    fn worker(role: &str) -> WorkerSpec {
        WorkerSpec {
            role: role.to_string(),
            spec: AgentSpec::builtin(AgentChoice::Codex, None),
            timeout_secs: None,
        }
    }

    fn output(stdout: &str, stderr: &str, timed_out: bool) -> RunOutcome {
        RunOutcome {
            exit_status: Some(0),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            timed_out,
            retryable: false,
            token_usage: None,
        }
    }

    // preview_for_event test relocated to swarm-kernel::format tests (P5-S2.5)

    #[test]
    fn build_manager_prompt_includes_worker_status_and_stderr() {
        let results = vec![(worker("qa"), 1, output("found bug", "warning", false))];
        let prompt = build_manager_prompt("audit", &results, None);

        assert!(prompt.contains("Original task:\naudit"));
        assert!(prompt.contains("--- worker: qa (codex) exit=1 timed_out=false gate=UNVERIFIED"));
        assert!(prompt
            .contains("flags=NONZERO_EXIT,NO_CITATIONS,MISSING_PACKET_SECTIONS,BLOCKER_SIGNAL"));
        assert!(prompt.contains("found bug"));
        assert!(prompt.contains("stderr:\nwarning"));
    }

    #[test]
    fn assess_worker_output_accepts_cited_compact_packet() {
        let stdout = "\
Findings
- `build_manager_prompt` caps worker output with citations: /path/to/swarm/crates/swarm-exec/src/synthesis.rs:199
Risks
- None.
Steps
- Keep the manager synthesis compact.
Blockers
- None.
Tests
- `cargo test -p swarm-exec`.
";

        let gate = assess_worker_output(0, false, stdout, "");

        assert!(gate.verified);
        assert!(!gate.has_blockers);
        assert!(gate.citation_count >= 1);
        assert_eq!(gate.evidence_gap_count, 0);
        assert!(gate.flags.is_empty());
    }

    #[test]
    fn assess_worker_output_allows_describing_unverified_state() {
        let stdout = "\
Findings
- The manager marks citation-free packets unverified: /path/to/swarm/crates/swarm-exec/src/synthesis.rs:46
Risks
- None.
Steps
- Keep gate language observable.
Blockers
- None.
Tests
- `cargo test -p swarm-exec`.
";

        let gate = assess_worker_output(0, false, stdout, "");

        assert!(gate.verified);
        assert_eq!(gate.evidence_gap_count, 0);
        assert!(gate.flags.is_empty());
    }

    #[test]
    fn assess_worker_output_flags_uncited_evidence_gaps_and_timeouts() {
        let stdout = "\
Findings
- This probably works.
Risks
- NEEDS_EVIDENCE for source anchor.
Steps
- Inspect runtime.
Blockers
- NEEDS_EVIDENCE.
Tests
- Run cargo test.
";

        let gate = assess_worker_output(124, true, stdout, "could not verify");

        assert!(!gate.verified);
        assert!(gate.has_blockers);
        assert!(gate.flags.contains(&"TIMED_OUT".to_string()));
        assert!(gate.flags.contains(&"NONZERO_EXIT".to_string()));
        assert!(gate.flags.contains(&"NO_CITATIONS".to_string()));
        assert!(gate.flags.contains(&"EVIDENCE_GAP".to_string()));
        assert!(gate.flags.contains(&"BLOCKER_SIGNAL".to_string()));
        assert!(gate.evidence_gap_count >= 2);
    }

    #[test]
    fn build_manager_prompt_caps_unverified_outputs_more_aggressively() {
        let long_tail = "tail-marker ".repeat(200);
        let stdout = format!(
            "Findings\n- Uncited claim.\nRisks\n- None.\nSteps\n- Inspect.\nBlockers\n- None.\nTests\n- Run tests.\n{long_tail}"
        );
        let results = vec![(worker("qa"), 0, output(&stdout, "", false))];
        let prompt = build_manager_prompt("audit", &results, None);

        assert!(prompt.contains("gate=UNVERIFIED"));
        assert!(prompt.contains("Runtime evidence gate: treat this worker as unverified"));
        assert!(prompt.contains("[output truncated to 450 bytes]"));
        assert!(!prompt.contains(&"tail-marker ".repeat(120)));
    }

    #[test]
    fn capped_manager_output_marks_oversized_synthesis() {
        let output = capped_manager_output(&"manager-tail ".repeat(400));

        assert!(output.contains("[output truncated to 3000 bytes]"));
        assert!(!output.contains(&"manager-tail ".repeat(260)));
    }

    #[test]
    fn build_discussion_manager_prompt_reports_turn_gate() {
        let turn = DiscussionTurn {
            round: 1,
            role: "qa".to_string(),
            spec: AgentSpec::builtin(AgentChoice::Codex, None),
            code: 0,
            timed_out: false,
            text: "\
Findings
- Verified source exists: /path/to/swarm/crates/swarm-exec/src/synthesis.rs:440
Risks
- None.
Steps
- Synthesize.
Blockers
- None.
Tests
- cargo test.
"
            .to_string(),
            stderr: String::new(),
        };

        let prompt = build_discussion_manager_prompt("task", "digest", &[turn]);

        assert!(prompt.contains("gate=VERIFIED"));
        assert!(prompt.contains("citations=1"));
        assert!(!prompt.contains("Runtime evidence gate: treat this turn as unverified"));
    }

    #[test]
    fn build_swarm_artifacts_handle_empty_outputs() {
        let manager = output("", "", false);
        let results = vec![(worker("architecture"), 0, output("", "", false))];

        let result = build_swarm_result_artifact(&results, &manager);
        let transcript = build_swarm_transcript("plan", &results, &manager);

        assert!(result.contains("(no manager output)"));
        assert!(result.contains("(no stdout)"));
        assert!(transcript.contains("# Agent Swarm Fan-Out"));
        assert!(transcript.contains("(no output)"));
    }

    #[test]
    fn build_discussion_digest_truncates_to_budget() {
        let turn = DiscussionTurn {
            round: 1,
            role: "architecture".to_string(),
            spec: AgentSpec::builtin(AgentChoice::Claude, Some("sonnet".to_string())),
            code: 0,
            timed_out: false,
            text: "x ".repeat(200),
            stderr: "warning text".to_string(),
        };

        let digest = build_discussion_digest("task", &[turn], 160);

        assert!(digest.contains("[Digest truncated to 160 bytes.]"));
    }

    // --- auto-context injection tests ---

    #[test]
    fn build_worker_prompt_off_path_contains_compact_packet_contract() {
        let task = "audit the runtime";
        let role = "qa";
        let with_none = build_worker_prompt(task, role, None);

        assert!(with_none.contains("You are the `qa` worker"));
        assert!(with_none.contains("Stay under 12 bullets total"));
        assert!(with_none.contains("Required sections, in order:"));
        assert!(with_none.contains("Blockers: 0-3 bullets"));
        assert!(with_none.contains("NEEDS_EVIDENCE"));
        assert!(with_none.contains("Task:\naudit the runtime"));
        assert!(!with_none.contains("--- local context"));
    }

    #[test]
    fn direct_persona_prompt_wraps_compact_manager() {
        let prompt =
            build_direct_persona_prompt("ship the thin manager", "compact-manager").unwrap();

        assert!(prompt.contains("compact metadirector"));
        assert!(prompt.contains("Maximum 650 words"));
        assert!(prompt.contains("NEEDS_EVIDENCE"));
        assert!(prompt
            .contains("never infer languages, modules, structs, classes, or files from memory"));
        assert!(prompt.contains("Rejected Claims"));
        assert!(prompt.contains("Direct task:\nship the thin manager"));
    }

    #[test]
    fn direct_persona_prompt_wraps_gemini_large_context_manager() {
        let prompt =
            build_direct_persona_prompt("plan from broad packet context", "gemini-manager")
                .unwrap();

        assert!(prompt.contains("Gemini large-context metadirector"));
        assert!(prompt.contains("Source Map"));
        assert!(prompt.contains("packetized context"));
        assert!(prompt.contains("Rejected Claims"));
        assert!(prompt.contains("Maximum 750 words"));
        assert!(prompt.contains("Direct task:\nplan from broad packet context"));
    }

    #[test]
    fn direct_persona_prompt_accepts_profile_roles_and_rejects_unknown() {
        let prompt = build_direct_persona_prompt("map risks", "systems-architect").unwrap();
        assert!(prompt.contains("`systems-architect` direct-agent profile"));
        assert!(prompt.contains("schema documented"));

        let err = build_direct_persona_prompt("map risks", "made-up-persona").unwrap_err();
        assert!(err.contains("unknown direct persona"));
    }

    #[test]
    fn helper_discussion_and_docs_prompts_use_compact_packet_contract() {
        let participant = worker("architecture");
        let helper = profiles::ProfileHelper {
            role: "scout",
            purpose: "find source anchors",
            agent: "gemini",
        };
        let helper_prompt = build_profile_helper_prompt("task", "ctx", 1, &participant, &helper);
        let discussion_prompt = build_discussion_turn_prompt("task", "ctx", 1, &participant, "");
        let docs_prompt = build_docs_prompt("task", "transcript", "synthesis");

        for prompt in [helper_prompt, discussion_prompt, docs_prompt] {
            assert!(prompt.contains("Compact handoff packet rules:"));
            assert!(prompt.contains("Findings, Risks, Steps, Blockers, Tests"));
            assert!(prompt.contains("NEEDS_EVIDENCE"));
        }
    }

    #[test]
    fn build_manager_prompt_off_path_produces_expected_structure_and_health_rules() {
        // OFF path (context = None): Original task section appears immediately,
        // no context block present.
        let results = vec![(worker("qa"), 0, output("looks good", "", false))];
        let prompt = build_manager_prompt("audit", &results, None);
        assert!(prompt.contains("Original task:\naudit\n\nWorker outputs:"));
        assert!(prompt.contains("Treat timed-out, nonzero-exit, truncated"));
        assert!(prompt.contains("return NEEDS_EVIDENCE"));
        assert!(prompt.contains("verdict, accepted facts, rejected/failed claims"));
        assert!(!prompt.contains("--- local context"));
    }

    #[test]
    fn build_worker_prompt_injects_context_block_after_task() {
        // ON path: context block is appended after the task text.
        let ctx =
            "--- local context (auto-gathered, cwd: /tmp) ---\nfoo.rs: bar\n--- end context ---\n";
        let prompt = build_worker_prompt("analyze", "architecture", Some(ctx));
        assert!(prompt.contains("Task:\nanalyze"));
        assert!(prompt.contains("--- local context"));
        assert!(prompt.contains("--- end context ---"));
        // Context must come AFTER the task, not before.
        let task_pos = prompt.find("Task:\nanalyze").unwrap();
        let ctx_pos = prompt.find("--- local context").unwrap();
        assert!(ctx_pos > task_pos);
    }

    #[test]
    fn build_manager_prompt_injects_context_before_worker_outputs() {
        // ON path: context block appears after task text, before worker outputs.
        let ctx =
            "--- local context (auto-gathered, cwd: /tmp) ---\nfoo.rs: bar\n--- end context ---\n";
        let results = vec![(worker("qa"), 0, output("all good", "", false))];
        let prompt = build_manager_prompt("plan", &results, Some(ctx));
        assert!(prompt.contains("--- local context"));
        let ctx_pos = prompt.find("--- local context").unwrap();
        let worker_pos = prompt.find("Worker outputs:").unwrap();
        assert!(ctx_pos < worker_pos);
    }

    #[test]
    fn render_context_block_returns_none_for_empty_symbols() {
        let ctx = serde_json::json!({
            "cwd": "/tmp",
            "symbols": []
        });
        assert!(render_context_block(&ctx).is_none());
    }

    #[test]
    fn render_context_block_formats_header_and_entries() {
        let ctx = serde_json::json!({
            "cwd": "/tmp/proj",
            "symbols": [
                {"path": "src/main.rs", "excerpt": "fn main() {}"},
                {"path": "README.md", "excerpt": "description"}
            ]
        });
        let block = render_context_block(&ctx).expect("should render non-empty symbols");
        assert!(block.starts_with("--- local context (auto-gathered, cwd: /tmp/proj) ---\n"));
        assert!(block.contains("src/main.rs: fn main() {}"));
        assert!(block.contains("README.md: description"));
        assert!(block.ends_with("--- end context ---\n"));
    }

    #[test]
    fn render_context_block_handles_missing_excerpt() {
        // A symbol with no excerpt renders as just the path with no trailing colon.
        let ctx = serde_json::json!({
            "cwd": "/tmp",
            "symbols": [{"path": "Cargo.toml"}]
        });
        let block = render_context_block(&ctx).expect("non-empty");
        assert!(block.contains("Cargo.toml\n"));
        assert!(!block.contains("Cargo.toml: "));
    }

    #[test]
    fn test_verify_metadirector_contract() {
        // A completely valid output
        let valid_text = "\
### Source Map
- [cli.rs](file:///path/to/swarm/crates/swarm-cli/src/cli.rs)

### Verdict
Everything looks good.

### Accepted Facts
The swarm uses MCP stdio services.

### Rejected Claims
No stale claims.

### Next Slice
Add tests.

### Tests
cargo test.
";
        assert!(verify_metadirector_contract(valid_text).is_ok());

        // Text with synonyms
        let synonym_text = "\
### Source-map
- [cli.rs](file:///path/to/swarm/crates/swarm-cli/src/cli.rs)

### What Changed
Some edits.

### Verification
Contract verified.

### Risks / Gaps
None.

### Next-slice
Implement verification.

### Tests
Run the verifier.
";
        assert!(verify_metadirector_contract(synonym_text).is_ok());

        // Text missing Source Map and Next Slice
        let invalid_text = "\
### Verdict
Some edits.
### Verification
Contract verified.
### Risks
None.
";
        let res = verify_metadirector_contract(invalid_text);
        assert!(res.is_err());
        let missing = res.unwrap_err();
        assert!(missing.contains(&"Source Map".to_string()));
        assert!(missing.contains(&"Next Slice".to_string()));
    }

    #[test]
    fn test_compact_handoff_contract_parity() {
        assert_eq!(
            COMPACT_HANDOFF_CONTRACT,
            swarm_kernel::prompts::COMPACT_HANDOFF_CONTRACT
        );
        assert!(COMPACT_HANDOFF_CONTRACT.contains("Findings: 1-4 bullets"));
        assert!(COMPACT_HANDOFF_CONTRACT.contains("NEEDS_EVIDENCE"));
        assert!(COMPACT_HANDOFF_CONTRACT.contains("Stop after the Tests section."));
    }
}
