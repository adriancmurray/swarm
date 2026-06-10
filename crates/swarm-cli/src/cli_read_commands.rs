//! Read/query style CLI commands for profiles, routing memory, and proposals.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::package_repo::{PackageRepo, StaticPackageRepo};
use crate::routing_repo::{RoutingMemory, RoutingMemoryRepo};
use serde::{Deserialize, Serialize};
use swarm_core::{LedgerRepo, LedgerStatus, LedgerTask};
use swarm_kernel::args::{load_default_timeout, print_help};
use swarm_kernel::conductor::{handle_hook_stdin, record_activity};
use swarm_kernel::config::load_config;
use swarm_kernel::format::json_text;
use swarm_kernel::resolver::agent_available;
use swarm_kernel::task_classifier::{
    classify_task, DEFAULT_CLASSIFIER_MODEL, DEFAULT_CLASSIFIER_PROVIDER,
};
use swarm_kernel::{profiles, telemetry};
use swarm_mcp::overview::overview_json;
use swarm_store::repos::ledger_repo::{default_file_ledger_repo, FileLedgerRepo};
use swarm_store::repos::telemetry_repo::{default_file_telemetry_repo, TelemetryRepo};
use swarm_store::store::now_ms;

const DEFAULT_MANAGER_PROMPT_LIMIT_TOKENS: u64 = 2_000;
const FULL_TRANSCRIPT_PROXY_TOKENS_PER_FIXTURE: u64 = 1_200;

// ── Ledger (T2 task ledger) ─────────────────────────────────────────────────────
//
// Append-only task ledger from the metadirector virtual-context design
// (`docs/agents/metadirector-virtual-context.md`). The `working-set` quantity
// is the token proxy a thin metadirector would hold for the currently-active
// (non verified-done) tasks — the primitive that lets the eval derive a
// working set from real ledger rows instead of fixture-suite size.

const LEDGER_PACKET_OVERHEAD_TOKENS: u64 = 24;

const LEDGER_USAGE: &str = "usage: agent-swarm ledger add --id ID --intent TEXT [--owner AGENT] [--depends-on ID]... [--status open|claimed|claimed_done|verified_done] [--anchor TEXT] [--dir PATH]\n\
     usage: agent-swarm ledger list [--json] [--dir PATH]\n\
     usage: agent-swarm ledger set-status --id ID --status open|claimed|claimed_done|verified_done [--anchor TEXT] [--dir PATH]\n\
     usage: agent-swarm ledger working-set [--dir PATH]\n\
     (ledger dir defaults to $SWARM_LEDGER_DIR, then $SWARM_HOME/ledger, then $HOME/.swarm/ledger)";

/// Bounded per-task packet token proxy: fixed overhead plus the intent's token
/// estimate (chars / 4, mirroring `estimate_tokens_text`).
fn task_token_proxy(task: &LedgerTask) -> u64 {
    let intent_tokens = (task.intent.chars().count() as u64).div_ceil(4);
    LEDGER_PACKET_OVERHEAD_TOKENS.saturating_add(intent_tokens)
}

/// Working-set token quantity: sum of bounded packet proxies over active (non
/// verified-done) tasks — the context a thin metadirector would hold at once.
fn ledger_working_set_tokens(tasks: &[LedgerTask]) -> u64 {
    tasks
        .iter()
        .filter(|t| t.status.is_active())
        .map(task_token_proxy)
        .fold(0u64, |acc, t| acc.saturating_add(t))
}

fn ledger_working_set_json(tasks: &[LedgerTask]) -> serde_json::Value {
    let active: Vec<&LedgerTask> = tasks.iter().filter(|t| t.status.is_active()).collect();
    let unverified_done = tasks.iter().filter(|t| t.is_unverified_done()).count();
    serde_json::json!({
        "schema": "agent-swarm/ledger-working-set/v1",
        "total_task_count": tasks.len(),
        "active_task_count": active.len(),
        "working_set_tokens": ledger_working_set_tokens(tasks),
        "unverified_done_count": unverified_done,
        "active_task_ids": active.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
    })
}

fn ledger_repo_from_dir(dir_override: Option<PathBuf>) -> Result<FileLedgerRepo, String> {
    if let Some(dir) = dir_override {
        return Ok(FileLedgerRepo::new(dir));
    }
    if let Some(dir) = std::env::var_os("SWARM_LEDGER_DIR") {
        return Ok(FileLedgerRepo::new(PathBuf::from(dir)));
    }
    default_file_ledger_repo().ok_or_else(|| {
        "Error: cannot resolve the swarm home (set SWARM_HOME or HOME); \
         cannot locate the ledger directory"
            .to_string()
    })
}

/// Loads a pinned ledger snapshot from an explicit path for the eval — kept
/// separate from the runtime `--dir` resolution so the eval stays deterministic.
/// A missing `--ledger` path is a loud error (not a silent empty snapshot), so a
/// typo'd path can never trivially satisfy the context gate. An existing but
/// task-less directory is a legitimate empty ledger.
fn load_ledger_snapshot(path: &Path) -> Result<Vec<LedgerTask>, String> {
    if !path.exists() {
        return Err(format!(
            "Error: --ledger path {} does not exist; pass an existing ledger directory",
            path.display()
        ));
    }
    FileLedgerRepo::new(path.to_path_buf())
        .tasks()
        .map_err(|err| format!("Error reading ledger snapshot {}: {err}", path.display()))
}

/// Loud invariant guard (spec lines 53-55, and `feedback_no_silent_fallbacks`):
/// `verified_done` requires a non-empty validation anchor, never a silent accept.
fn validate_verified_done_loud(status: LedgerStatus, anchor: Option<&str>) -> Result<(), String> {
    if status == LedgerStatus::VerifiedDone && anchor.map(|a| a.trim().is_empty()).unwrap_or(true) {
        return Err(
            "Error: verified_done requires a non-empty --anchor (validation anchor); \
             the task stays claimed_done until a verification anchor is supplied"
                .to_string(),
        );
    }
    Ok(())
}

#[derive(Default)]
struct LedgerArgs {
    dir: Option<PathBuf>,
    id: Option<String>,
    intent: Option<String>,
    owner: Option<String>,
    status: Option<String>,
    anchor: Option<String>,
    depends_on: Vec<String>,
    json: bool,
}

fn ledger_next_val(iter: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, String> {
    iter.next()
        .cloned()
        .ok_or_else(|| format!("Error: {flag} requires a value"))
}

fn parse_ledger_args(raw: &[String]) -> Result<LedgerArgs, String> {
    let mut parsed = LedgerArgs::default();
    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--dir" => parsed.dir = Some(PathBuf::from(ledger_next_val(&mut iter, "--dir")?)),
            "--id" => parsed.id = Some(ledger_next_val(&mut iter, "--id")?),
            "--intent" => parsed.intent = Some(ledger_next_val(&mut iter, "--intent")?),
            "--owner" | "--owner-agent" => {
                parsed.owner = Some(ledger_next_val(&mut iter, "--owner")?)
            }
            "--status" => parsed.status = Some(ledger_next_val(&mut iter, "--status")?),
            "--anchor" | "--validation-anchor" => {
                parsed.anchor = Some(ledger_next_val(&mut iter, "--anchor")?)
            }
            "--depends-on" | "--depends" => parsed
                .depends_on
                .push(ledger_next_val(&mut iter, "--depends-on")?),
            "--json" => parsed.json = true,
            _ if arg.starts_with("--dir=") => {
                parsed.dir = Some(PathBuf::from(&arg["--dir=".len()..]))
            }
            _ if arg.starts_with("--id=") => parsed.id = Some(arg["--id=".len()..].to_string()),
            _ if arg.starts_with("--intent=") => {
                parsed.intent = Some(arg["--intent=".len()..].to_string())
            }
            _ if arg.starts_with("--owner=") => {
                parsed.owner = Some(arg["--owner=".len()..].to_string())
            }
            _ if arg.starts_with("--status=") => {
                parsed.status = Some(arg["--status=".len()..].to_string())
            }
            _ if arg.starts_with("--anchor=") => {
                parsed.anchor = Some(arg["--anchor=".len()..].to_string())
            }
            _ if arg.starts_with("--depends-on=") => parsed
                .depends_on
                .push(arg["--depends-on=".len()..].to_string()),
            other => {
                return Err(format!(
                    "Error: unknown ledger option `{other}`\n{LEDGER_USAGE}"
                ))
            }
        }
    }
    Ok(parsed)
}

fn parse_ledger_status(token: &str) -> Result<LedgerStatus, String> {
    LedgerStatus::parse(token).ok_or_else(|| {
        format!("Error: unknown status `{token}` (open|claimed|claimed_done|verified_done)")
    })
}

fn task_json(task: &LedgerTask) -> Result<serde_json::Value, String> {
    serde_json::to_value(task).map_err(|err| format!("Error serializing ledger task: {err}"))
}

pub fn cmd_ledger(raw: &[String]) -> Result<i32, String> {
    let Some(sub) = raw.first() else {
        return Err(LEDGER_USAGE.to_string());
    };
    let rest = &raw[1..];
    match sub.as_str() {
        "add" => ledger_add(rest),
        "list" => ledger_list(rest),
        "set-status" => ledger_set_status(rest),
        "working-set" => ledger_working_set_cmd(rest),
        "--help" | "-h" => Err(LEDGER_USAGE.to_string()),
        other => Err(format!(
            "Error: unknown ledger subcommand `{other}`\n{LEDGER_USAGE}"
        )),
    }
}

fn ledger_add(raw: &[String]) -> Result<i32, String> {
    let args = parse_ledger_args(raw)?;
    let id = args.id.ok_or("Error: ledger add requires --id")?;
    let intent = args.intent.ok_or("Error: ledger add requires --intent")?;
    let repo = ledger_repo_from_dir(args.dir)?;

    let mut task = LedgerTask::new(id, intent, now_ms());
    task.owner_agent = args.owner;
    task.depends_on = args.depends_on;
    if let Some(status_str) = args.status.as_deref() {
        let status = parse_ledger_status(status_str)?;
        validate_verified_done_loud(status, args.anchor.as_deref())?;
        task.status = status;
    }
    task.validation_anchor = args.anchor;
    if !task.status.is_active() {
        task.closed_at_ms = Some(now_ms());
    }

    repo.record_task(task.clone())
        .map_err(|err| err.to_string())?;
    println!(
        "{}",
        json_text(serde_json::json!({
            "schema": "agent-swarm/ledger-add/v1",
            "recorded": task_json(&task)?,
        }))
    );
    Ok(0)
}

fn ledger_list(raw: &[String]) -> Result<i32, String> {
    let args = parse_ledger_args(raw)?;
    let repo = ledger_repo_from_dir(args.dir)?;
    let tasks = repo.tasks().map_err(|err| err.to_string())?;
    let rows = tasks.iter().map(task_json).collect::<Result<Vec<_>, _>>()?;
    println!(
        "{}",
        json_text(serde_json::json!({
            "schema": "agent-swarm/ledger-list/v1",
            "task_count": tasks.len(),
            "tasks": rows,
        }))
    );
    Ok(0)
}

fn ledger_set_status(raw: &[String]) -> Result<i32, String> {
    let args = parse_ledger_args(raw)?;
    let id = args.id.ok_or("Error: ledger set-status requires --id")?;
    let status_str = args
        .status
        .as_deref()
        .ok_or("Error: ledger set-status requires --status")?;
    let status = parse_ledger_status(status_str)?;
    let repo = ledger_repo_from_dir(args.dir)?;
    let tasks = repo.tasks().map_err(|err| err.to_string())?;
    let mut task = tasks
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("Error: no ledger task with id `{id}`"))?;

    // A new --anchor wins; otherwise keep any existing anchor on the task.
    let anchor = args.anchor.or_else(|| task.validation_anchor.clone());
    validate_verified_done_loud(status, anchor.as_deref())?;
    task.status = status;
    task.validation_anchor = anchor;
    if !status.is_active() {
        task.closed_at_ms = Some(now_ms());
    }

    repo.record_task(task.clone())
        .map_err(|err| err.to_string())?;
    println!(
        "{}",
        json_text(serde_json::json!({
            "schema": "agent-swarm/ledger-set-status/v1",
            "updated": task_json(&task)?,
        }))
    );
    Ok(0)
}

fn ledger_working_set_cmd(raw: &[String]) -> Result<i32, String> {
    let args = parse_ledger_args(raw)?;
    let repo = ledger_repo_from_dir(args.dir)?;
    let tasks = repo.tasks().map_err(|err| err.to_string())?;
    println!("{}", json_text(ledger_working_set_json(&tasks)));
    Ok(0)
}

#[cfg(test)]
mod ledger_cmd_tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agent-swarm-ledger-cmd-{}-{}-{}",
            tag,
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn parse_args_supports_space_equals_and_repeats() {
        let parsed = parse_ledger_args(&[
            "--id".to_string(),
            "t-1".to_string(),
            "--intent=do the thing".to_string(),
            "--owner".to_string(),
            "gemini".to_string(),
            "--depends-on".to_string(),
            "a".to_string(),
            "--depends-on=b".to_string(),
            "--json".to_string(),
        ])
        .unwrap();
        assert_eq!(parsed.id.as_deref(), Some("t-1"));
        assert_eq!(parsed.intent.as_deref(), Some("do the thing"));
        assert_eq!(parsed.owner.as_deref(), Some("gemini"));
        assert_eq!(parsed.depends_on, vec!["a".to_string(), "b".to_string()]);
        assert!(parsed.json);
    }

    #[test]
    fn parse_args_rejects_unknown_option() {
        assert!(parse_ledger_args(&["--bogus".to_string()]).is_err());
    }

    #[test]
    fn verified_done_requires_anchor_loud() {
        assert!(validate_verified_done_loud(LedgerStatus::VerifiedDone, None).is_err());
        assert!(validate_verified_done_loud(LedgerStatus::VerifiedDone, Some("   ")).is_err());
        assert!(validate_verified_done_loud(LedgerStatus::VerifiedDone, Some("test:foo")).is_ok());
        assert!(validate_verified_done_loud(LedgerStatus::ClaimedDone, None).is_ok());
    }

    #[test]
    fn working_set_tokens_count_only_active() {
        let open = LedgerTask::new("t-1", "alpha beta gamma", 1);
        let mut verified = LedgerTask::new("t-2", "delta", 2);
        verified.status = LedgerStatus::VerifiedDone;
        verified.validation_anchor = Some("test:x".to_string());
        let tasks = vec![open, verified];
        assert_eq!(
            ledger_working_set_tokens(&tasks),
            task_token_proxy(&tasks[0])
        );
    }

    #[test]
    fn add_set_status_roundtrip_enforces_anchor_via_dir() {
        let dir = temp_dir("roundtrip");
        let d = dir.display().to_string();

        assert_eq!(
            ledger_add(&[
                "--dir".to_string(),
                d.clone(),
                "--id".to_string(),
                "t-1".to_string(),
                "--intent".to_string(),
                "build the thing".to_string(),
            ])
            .unwrap(),
            0
        );
        ledger_add(&[
            "--dir".to_string(),
            d.clone(),
            "--id".to_string(),
            "t-2".to_string(),
            "--intent".to_string(),
            "review the thing".to_string(),
        ])
        .unwrap();

        let repo = ledger_repo_from_dir(Some(dir.clone())).unwrap();
        assert_eq!(repo.tasks().unwrap().len(), 2);

        // verified_done without anchor is a loud error, leaving state unchanged.
        let err = ledger_set_status(&[
            "--dir".to_string(),
            d.clone(),
            "--id".to_string(),
            "t-1".to_string(),
            "--status".to_string(),
            "verified_done".to_string(),
        ])
        .unwrap_err();
        assert!(err.contains("verified_done requires"));
        assert_eq!(
            repo.tasks()
                .unwrap()
                .iter()
                .filter(|t| t.status.is_active())
                .count(),
            2,
            "rejected transition must not change state"
        );

        // With an anchor it succeeds and leaves the working set.
        ledger_set_status(&[
            "--dir".to_string(),
            d.clone(),
            "--id".to_string(),
            "t-1".to_string(),
            "--status".to_string(),
            "verified_done".to_string(),
            "--anchor".to_string(),
            "test:foo".to_string(),
        ])
        .unwrap();
        let tasks = repo.tasks().unwrap();
        assert_eq!(
            tasks.iter().filter(|t| t.status.is_active()).count(),
            1,
            "verified task leaves the working set"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    fn write_min_fixtures(dir: &Path) -> PathBuf {
        let path = dir.join("fixtures.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "schema": "swarm/metadirector-eval-fixtures/v1",
                "fixtures": [{
                    "id": "ui-1",
                    "task": "Fix graph node layout",
                    "expected_classification": "ui-design",
                    "must_touch": ["packages/panels/graph_panel"],
                    "must_not_touch": ["mesh/crates/secret-vault"],
                    "quality_checks": ["Graph layout is stable."],
                    "escalation_expected": false
                }]
            })
            .to_string(),
        )
        .unwrap();
        path
    }

    #[test]
    fn eval_ledger_sources_working_set_from_pinned_snapshot() {
        let dir = temp_dir("eval-ledger");
        let repo = ledger_repo_from_dir(Some(dir.clone())).unwrap();
        repo.record_task(LedgerTask::new("a", "task alpha", 1))
            .unwrap();
        repo.record_task(LedgerTask::new("b", "task beta", 2))
            .unwrap();
        let mut verified = LedgerTask::new("c", "task gamma", 3);
        verified.status = LedgerStatus::VerifiedDone;
        verified.validation_anchor = Some("test:done".to_string());
        repo.record_task(verified).unwrap();

        let fixtures = write_min_fixtures(&dir);
        let payload = eval_metadirector_payload(&[
            "--arm=all".to_string(),
            "--classifier=deterministic".to_string(),
            "--packet-budget=300".to_string(),
            "--manager-prompt-limit=2000".to_string(),
            "--ledger".to_string(),
            dir.display().to_string(),
            "--no-write-summary".to_string(),
            "--fixtures".to_string(),
            fixtures.display().to_string(),
        ])
        .unwrap();

        // The context gate now reflects real ledger rows, not fixture-suite size.
        assert_eq!(payload["manager_prompt_source"], "ledger");
        assert_eq!(payload["ledger_active_task_count"], 2);
        let tasks = repo.tasks().unwrap();
        let active = tasks.iter().filter(|t| t.status.is_active()).count() as u64;
        let expected = ledger_working_set_tokens(&tasks)
            .saturating_add(manager_prompt_overhead_tokens(active));
        assert_eq!(
            payload["manager_prompt_estimated_tokens"].as_u64().unwrap(),
            expected
        );
        assert_eq!(payload["manager_prompt_within_limit"], true);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_ledger_snapshot_missing_path_is_loud_error() {
        let err = load_ledger_snapshot(&PathBuf::from(
            "/tmp/agent-swarm-ledger-definitely-missing-xyz-123",
        ))
        .unwrap_err();
        assert!(err.contains("does not exist"), "got: {err}");
    }

    #[test]
    fn load_ledger_snapshot_existing_empty_dir_is_ok() {
        let dir = temp_dir("empty");
        assert!(load_ledger_snapshot(&dir).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}

pub(crate) fn cmd_overview() -> Result<i32, String> {
    let json = serde_json::to_string_pretty(&overview_json()?)
        .map_err(|err| format!("Error serializing overview: {err}"))?;
    println!("{json}");
    Ok(0)
}

pub(crate) fn cmd_insights() -> Result<i32, String> {
    println!("{}", json_text(insights_payload()));
    Ok(0)
}

pub(crate) fn cmd_profiles() -> Result<i32, String> {
    println!("{}", json_text(profiles_payload()));
    Ok(0)
}

pub(crate) fn cmd_automation_hooks() -> Result<i32, String> {
    println!("{}", json_text(automation_hooks_payload()));
    Ok(0)
}

pub(crate) fn cmd_presets() -> Result<i32, String> {
    println!("{}", json_text(presets_payload()));
    Ok(0)
}

pub(crate) fn cmd_manifest() -> Result<i32, String> {
    let payload = StaticPackageRepo
        .manifest()
        .map_err(|err| format!("Error loading manifest: {err}"))?;
    let json = serde_json::to_string_pretty(&payload)
        .map_err(|err| format!("Error serializing manifest: {err}"))?;
    println!("{json}");
    Ok(0)
}

pub(crate) fn cmd_conductor_hook() -> Result<i32, String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("Error reading conductor hook stdin: {err}"))?;
    if let Some(decision) = handle_hook_stdin(&input) {
        print!(
            "{}",
            serde_json::to_string(&decision).unwrap_or_else(|_| decision.to_string())
        );
    }
    Ok(0)
}

pub(crate) fn cmd_activity_record(raw: &[String]) -> Result<i32, String> {
    let args = parse_activity_record_args(raw)?;
    let result = record_activity(&args)?;
    println!(
        "{}",
        json_text(serde_json::json!({
            "schema": "agent-swarm/activity-record-result/v1",
            "session_id": result.session_id,
            "node_id": result.node_id,
            "path": result.path.display().to_string(),
        }))
    );
    Ok(0)
}

pub(crate) fn cmd_recommend(raw: &[String]) -> Result<i32, String> {
    println!("{}", json_text(recommend_payload(raw)?));
    Ok(0)
}

pub(crate) fn cmd_eval_metadirector(raw: &[String]) -> Result<i32, String> {
    println!("{}", json_text(eval_metadirector_payload(raw)?));
    Ok(0)
}

pub(crate) fn cmd_feedback(raw: &[String]) -> Result<i32, String> {
    let args = match parse_feedback_args(raw)? {
        FeedbackParse::Help => return Ok(0),
        FeedbackParse::Args(args) => args,
    };
    println!(
        "{}",
        json_text(telemetry::feedback_json(
            args.session_id,
            args.role,
            args.agent,
            args.outcome,
            args.note,
        )?)
    );
    Ok(0)
}

pub(crate) fn cmd_proposals() -> Result<i32, String> {
    println!("{}", json_text(proposals_payload()));
    Ok(0)
}

pub(crate) fn cmd_propose(raw: &[String]) -> Result<i32, String> {
    let args = parse_propose_args(raw)?;
    println!(
        "{}",
        json_text(telemetry::proposal_json(
            args.session_id,
            args.title,
            args.body,
            args.proposed_by,
            args.tags,
        )?)
    );
    Ok(0)
}

pub(crate) fn cmd_proposal_vote(raw: &[String]) -> Result<i32, String> {
    let args = parse_proposal_vote_args(raw)?;
    println!(
        "{}",
        json_text(telemetry::proposal_vote_json(
            args.proposal_id.into(),
            args.voter,
            args.vote,
            args.rationale,
        )?)
    );
    Ok(0)
}

fn insights_payload() -> serde_json::Value {
    // S6: route agents/recommendations through RoutingMemory<FileTelemetryRepo>.
    // The remaining fields (store path, counts, proposals) are read from the same
    // underlying repo to preserve byte-identity.
    //
    // When HOME is unset we fall through to RoutingMemory<MemTelemetryRepo> with
    // no data so the payload shape is identical to the original (empty data,
    // store=null, default-agent recommendations).
    use swarm_store::repos::telemetry_repo::MemTelemetryRepo;

    match default_file_telemetry_repo() {
        Some(repo) => {
            let store_path = repo.dir().display().to_string();
            let proposals = repo.proposals().unwrap_or_default();
            let votes = repo.proposal_votes().unwrap_or_default();
            let observations = repo.observations().unwrap_or_default();
            let feedback = repo.feedback().unwrap_or_default();
            let routing = RoutingMemory::new(repo);
            let stats = routing.agent_stats().unwrap_or_default();
            let recs: Vec<serde_json::Value> = routing
                .recommendations()
                .unwrap_or_default()
                .into_iter()
                .map(|r| serde_json::json!({"role": r.role, "agent": r.agent}))
                .collect();
            serde_json::json!({
                "schema": "agent-swarm/insights/v1",
                "store": store_path,
                "observation_count": observations.len(),
                "feedback_count": feedback.len(),
                "proposal_count": proposals.len(),
                "proposal_vote_count": votes.len(),
                "agents": stats,
                "recommendations": recs,
                "proposals": telemetry::proposal_summaries(&proposals, &votes),
            })
        }
        None => {
            // HOME unset: same payload shape as original (empty data, store=null,
            // default-agent recommendations via empty RoutingMemory).
            let routing = RoutingMemory::new(MemTelemetryRepo::new());
            let recs: Vec<serde_json::Value> = routing
                .recommendations()
                .unwrap_or_default()
                .into_iter()
                .map(|r| serde_json::json!({"role": r.role, "agent": r.agent}))
                .collect();
            serde_json::json!({
                "schema": "agent-swarm/insights/v1",
                "store": serde_json::Value::Null,
                "observation_count": 0usize,
                "feedback_count": 0usize,
                "proposal_count": 0usize,
                "proposal_vote_count": 0usize,
                "agents": serde_json::Value::Array(vec![]),
                "recommendations": recs,
                "proposals": serde_json::Value::Array(vec![]),
            })
        }
    }
}

fn parse_activity_record_args(raw: &[String]) -> Result<serde_json::Value, String> {
    let mut object = serde_json::Map::new();
    let mut i = 0;
    while i < raw.len() {
        let arg = &raw[i];
        if arg == "--json" {
            let value = raw
                .get(i + 1)
                .ok_or_else(|| "--json requires a JSON object".to_string())?;
            let decoded: serde_json::Value = serde_json::from_str(value)
                .map_err(|err| format!("Error parsing --json activity record: {err}"))?;
            if !decoded.is_object() {
                return Err("--json requires a JSON object".into());
            }
            return Ok(decoded);
        }
        let key = match arg.as_str() {
            "--session" | "--session-id" => "session_id",
            "--node" | "--node-id" => "node_id",
            "--parent" | "--parent-id" => "parent_id",
            "--depth" => "depth",
            "--label" => "label",
            "--status" => "status",
            "--cwd" => "cwd",
            "--agent-type" => "agent_type",
            "--prompt-preview" => "prompt_preview",
            "--cullable-handle" => "cullable_handle",
            "--policy-state" => "policy_state",
            "--slice-id" => "slice_id",
            "--tool-use-id" => "tool_use_id",
            "--agent-id" => "agent_id",
            other => return Err(format!("unknown activity-record argument: {other}")),
        };
        let value = raw
            .get(i + 1)
            .ok_or_else(|| format!("{arg} requires a value"))?;
        if key == "depth" {
            object.insert(
                key.into(),
                serde_json::json!(value
                    .parse::<i64>()
                    .map_err(|_| { format!("{arg} requires an integer value") })?),
            );
        } else {
            object.insert(key.into(), serde_json::json!(value));
        }
        i += 2;
    }
    Ok(serde_json::Value::Object(object))
}

fn profiles_payload() -> serde_json::Value {
    let repo = StaticPackageRepo;
    let profile_list = repo
        .profiles()
        .expect("StaticPackageRepo::profiles is infallible");
    serde_json::json!({
        "schema": "agent-swarm/profiles/v1",
        "profiles": profile_list,
    })
}

fn automation_hooks_payload() -> serde_json::Value {
    profiles::automation_hooks_json()
}

fn presets_payload() -> serde_json::Value {
    StaticPackageRepo
        .presets()
        .expect("StaticPackageRepo::presets is infallible")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClassifierMode {
    Auto,
    Deterministic,
    Semantic,
}

impl ClassifierMode {
    fn from_str(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "deterministic" | "rust" => Ok(Self::Deterministic),
            "semantic" | "gemma" | "mlx" => Ok(Self::Semantic),
            other => Err(format!(
                "Error: unknown classifier mode `{other}` (expected auto, deterministic, or semantic)"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Deterministic => "deterministic",
            Self::Semantic => "semantic",
        }
    }
}

impl Default for ClassifierMode {
    fn default() -> Self {
        std::env::var("AGENT_SWARM_CLASSIFIER")
            .ok()
            .and_then(|value| Self::from_str(value.trim()).ok())
            .unwrap_or(Self::Auto)
    }
}

struct RecommendArgs {
    task: String,
    classifier_mode: ClassifierMode,
    classifier_threshold: u8,
    mlx_endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct EffectiveClassification {
    task_type: String,
    confidence: u8,
    roles: Vec<String>,
    classifier: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
struct SemanticClassification {
    task_type: String,
    confidence: u8,
    #[serde(default)]
    reason: Option<String>,
}

fn recommend_payload(raw: &[String]) -> Result<serde_json::Value, String> {
    let recommend_args = parse_recommend_args(raw)?;
    let task = recommend_args.task;
    // S6: route per-role best-agent lookups through RoutingMemory<FileTelemetryRepo>.
    let classification = effective_classification(
        &task,
        recommend_args.classifier_mode,
        recommend_args.classifier_threshold,
        recommend_args.mlx_endpoint.as_deref(),
    );

    use swarm_store::repos::telemetry_repo::MemTelemetryRepo;

    let (observation_count, feedback_count, routing): (usize, usize, Box<dyn RoutingMemoryRepo>) =
        match default_file_telemetry_repo() {
            Some(repo) => {
                let observations = repo.observations().unwrap_or_default();
                let feedback = repo.feedback().unwrap_or_default();
                let obs_len = observations.len();
                let fb_len = feedback.len();
                (obs_len, fb_len, Box::new(RoutingMemory::new(repo)))
            }
            None => {
                // HOME unset: empty RoutingMemory so best_agent_for_role falls
                // back to the same defaults as the original recommendation_json().
                (0, 0, Box::new(RoutingMemory::new(MemTelemetryRepo::new())))
            }
        };

    let config = load_config();
    let manager = config
        .swarm
        .default_manager
        .clone()
        .or(config.discussion.default_manager.clone())
        .or_else(|| routing.best_agent_for_role("manager").ok())
        .unwrap_or_else(|| "gemini".to_string());
    let participants: Vec<String> = classification
        .roles
        .iter()
        .map(|role| {
            let agent = preferred_agent_for_role(&config, role)
                .or_else(|| routing.best_agent_for_role(role).ok())
                .unwrap_or_else(|| "gemini".to_string());
            format!("{role}={agent}")
        })
        .collect();

    let task_preview: String = {
        let compact = task.split_whitespace().collect::<Vec<_>>().join(" ");
        if compact.chars().count() <= 160 {
            compact
        } else {
            format!("{}...", compact.chars().take(157).collect::<String>())
        }
    };

    Ok(serde_json::json!({
        "schema": "agent-swarm/recommendation/v1",
        "task_preview": task_preview,
        "classification": classification,
        "manager": manager,
        "participants": participants,
        "basis": {
            "observation_count": observation_count,
            "feedback_count": feedback_count,
            "strategy": "shared deterministic task classification plus lazy aggregate success/duration scoring and explicit user feedback"
        }
    }))
}

fn parse_recommend_args(raw: &[String]) -> Result<RecommendArgs, String> {
    let mut classifier_mode = ClassifierMode::default();
    let mut classifier_threshold = 80u8;
    let mut mlx_endpoint: Option<String> = None;
    let mut task_parts: Vec<String> = Vec::new();
    let mut iter = raw.iter().peekable();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--classifier" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --classifier requires a value".to_string())?;
                classifier_mode = ClassifierMode::from_str(value)?;
            }
            "--classifier-threshold" | "--mlx-threshold" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("Error: {arg} requires a value"))?;
                classifier_threshold = parse_classifier_threshold(value)?;
            }
            "--mlx-endpoint" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --mlx-endpoint requires a value".to_string())?;
                mlx_endpoint = Some(value.clone());
            }
            "--" => {
                task_parts.extend(iter.cloned());
                break;
            }
            "--help" | "-h" => {
                return Err(
                    "usage: agent-swarm recommend [--classifier auto|deterministic|semantic] [--classifier-threshold N] [--mlx-endpoint URL] \"<task>\""
                        .to_string(),
                );
            }
            _ if arg.starts_with("--classifier=") => {
                classifier_mode = ClassifierMode::from_str(&arg["--classifier=".len()..])?;
            }
            _ if arg.starts_with("--classifier-threshold=") => {
                classifier_threshold =
                    parse_classifier_threshold(&arg["--classifier-threshold=".len()..])?;
            }
            _ if arg.starts_with("--mlx-threshold=") => {
                classifier_threshold =
                    parse_classifier_threshold(&arg["--mlx-threshold=".len()..])?;
            }
            _ if arg.starts_with("--mlx-endpoint=") => {
                mlx_endpoint = Some(arg["--mlx-endpoint=".len()..].to_string());
            }
            _ if arg.starts_with("--") && task_parts.is_empty() => {
                return Err(format!("Error: unknown recommend option `{arg}`"));
            }
            _ => {
                task_parts.push(arg.clone());
                task_parts.extend(iter.cloned());
                break;
            }
        }
    }

    let task = task_parts.join(" ");
    if task.trim().is_empty() {
        return Err("Error: recommend requires a task description".to_string());
    }

    Ok(RecommendArgs {
        task,
        classifier_mode,
        classifier_threshold,
        mlx_endpoint,
    })
}

fn parse_classifier_threshold(value: &str) -> Result<u8, String> {
    let threshold = value
        .parse::<u8>()
        .map_err(|_| "Error: classifier threshold must be an integer 0-100".to_string())?;
    if threshold > 100 {
        Err("Error: classifier threshold must be between 0 and 100".to_string())
    } else {
        Ok(threshold)
    }
}

fn effective_classification(
    task: &str,
    mode: ClassifierMode,
    semantic_threshold: u8,
    mlx_endpoint: Option<&str>,
) -> EffectiveClassification {
    let deterministic = classify_task(task);
    let deterministic_roles = deterministic
        .roles
        .iter()
        .map(|role| (*role).to_string())
        .collect::<Vec<_>>();
    let mut effective = EffectiveClassification {
        task_type: deterministic.task_type.to_string(),
        confidence: deterministic.confidence,
        roles: deterministic_roles,
        classifier: serde_json::json!({
            "provider": DEFAULT_CLASSIFIER_PROVIDER,
            "model": DEFAULT_CLASSIFIER_MODEL,
            "mode": match mode {
                ClassifierMode::Auto => "auto-semantic-fallback",
                ClassifierMode::Deterministic => "deterministic-rust-fallback",
                ClassifierMode::Semantic => "semantic-mlx-forced",
            },
            "status": match mode {
                ClassifierMode::Auto if deterministic.confidence >= semantic_threshold => "deterministic-high-confidence",
                ClassifierMode::Deterministic => "deterministic-rust-fallback",
                _ => "gemma-mlx-pending",
            },
            "invoked": false,
            "threshold": semantic_threshold,
            "endpoint": mlx_endpoint,
            "deterministic": {
                "task_type": deterministic.task_type,
                "confidence": deterministic.confidence,
                "roles": deterministic.roles,
            }
        }),
    };

    let should_invoke = match mode {
        ClassifierMode::Deterministic => false,
        ClassifierMode::Semantic => true,
        ClassifierMode::Auto => deterministic.confidence < semantic_threshold,
    };
    if !should_invoke {
        return effective;
    }

    match classify_with_gemma_mlx(task, mlx_endpoint) {
        Ok(semantic) => {
            let valid_type = roles_for_task_type(&semantic.task_type).is_some();
            if valid_type {
                let accept = mode == ClassifierMode::Semantic
                    || semantic.confidence >= 70
                    || semantic.confidence >= deterministic.confidence;
                if accept {
                    effective.task_type = semantic.task_type.clone();
                    effective.confidence = semantic.confidence;
                    effective.roles = roles_for_task_type(&semantic.task_type)
                        .unwrap_or_default()
                        .into_iter()
                        .map(str::to_string)
                        .collect();
                }
                classifier_object_mut(&mut effective.classifier).extend([
                    ("status".to_string(), serde_json::json!("gemma-mlx-invoked")),
                    ("invoked".to_string(), serde_json::json!(true)),
                    ("accepted".to_string(), serde_json::json!(accept)),
                    (
                        "semantic".to_string(),
                        serde_json::json!({
                            "task_type": semantic.task_type,
                            "confidence": semantic.confidence,
                            "reason": semantic.reason,
                        }),
                    ),
                ]);
            } else {
                classifier_object_mut(&mut effective.classifier).extend([
                    (
                        "status".to_string(),
                        serde_json::json!("gemma-mlx-invalid-classification"),
                    ),
                    ("invoked".to_string(), serde_json::json!(true)),
                    (
                        "error".to_string(),
                        serde_json::json!(format!(
                            "unknown semantic task_type `{}`",
                            semantic.task_type
                        )),
                    ),
                ]);
            }
        }
        Err(err) => {
            classifier_object_mut(&mut effective.classifier).extend([
                (
                    "status".to_string(),
                    serde_json::json!("gemma-mlx-unavailable"),
                ),
                ("invoked".to_string(), serde_json::json!(true)),
                ("error".to_string(), serde_json::json!(err)),
            ]);
        }
    }

    effective
}

fn classifier_object_mut(
    classifier: &mut serde_json::Value,
) -> &mut serde_json::Map<String, serde_json::Value> {
    classifier.as_object_mut().expect("classifier is an object")
}

fn roles_for_task_type(task_type: &str) -> Option<Vec<&'static str>> {
    match task_type {
        "model-provider" => Some(vec!["architecture", "implementation-plan", "review"]),
        "docs" => Some(vec!["api-docs", "examples"]),
        "audit" => Some(vec!["architecture", "simplify", "hardening"]),
        "ui-design" => Some(vec![
            "product-design",
            "motion-accessibility",
            "component-architecture",
        ]),
        "implementation" => Some(vec!["architecture", "implementation-plan", "review"]),
        _ => None,
    }
}

/// Optional semantic-classifier escape hatch.
///
/// The engine ships no built-in semantic classifier backend, so this always
/// returns `Err` and the `recommend` command degrades to the deterministic
/// Rust classifier (`classify_task`). A private deployment that wants a
/// model-backed classifier wires a backend descriptor in its own config and
/// reimplements this hook; it is intentionally not part of the OSS engine.
fn classify_with_gemma_mlx(
    _task: &str,
    _mlx_endpoint: Option<&str>,
) -> Result<SemanticClassification, String> {
    Err("semantic classifier backend not configured".to_string())
}

/// Parse a JSON classification envelope. Retained for an external semantic
/// classifier integration; only exercised by tests in the OSS build, where no
/// classifier backend is wired.
#[cfg(test)]
fn parse_semantic_classifier_response(raw: &str) -> Result<SemanticClassification, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("semantic classifier returned empty output".to_string());
    }
    let json_slice = extract_json_object(trimmed).unwrap_or(trimmed);
    serde_json::from_str::<SemanticClassification>(json_slice)
        .map_err(|err| format!("could not parse semantic classifier JSON: {err}"))
}

#[cfg(test)]
fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if start <= end {
        Some(&raw[start..=end])
    } else {
        None
    }
}

fn preferred_agent_for_role(
    config: &swarm_kernel::config::SwarmConfig,
    role: &str,
) -> Option<String> {
    config.routes.get(role).and_then(|route| {
        route.preferred.iter().find_map(|spec| {
            let parsed = swarm_kernel::args::parse_agent_spec_struct(spec).ok()?;
            if agent_available(parsed.agent) {
                Some(spec.clone())
            } else {
                None
            }
        })
    })
}

#[derive(Debug, Deserialize)]
struct EvalFixtureFile {
    fixtures: Vec<EvalFixture>,
}

#[derive(Debug, Deserialize)]
struct EvalFixture {
    id: String,
    task: String,
    expected_classification: String,
    #[serde(default)]
    must_touch: Vec<String>,
    #[serde(default)]
    must_not_touch: Vec<String>,
    #[serde(default)]
    quality_checks: Vec<String>,
    #[serde(default)]
    escalation_expected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvalArm {
    Classifier,
    Packet,
    All,
}

impl EvalArm {
    fn from_str(value: &str) -> Result<Self, String> {
        match value {
            "classifier" | "routing" => Ok(Self::Classifier),
            "packet" | "packets" => Ok(Self::Packet),
            "all" => Ok(Self::All),
            other => Err(format!(
                "Error: unknown eval arm `{other}` (expected classifier, packet, or all)"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Classifier => "classifier",
            Self::Packet => "packet",
            Self::All => "all",
        }
    }

    fn includes_classifier(self) -> bool {
        matches!(self, Self::Classifier | Self::All)
    }

    fn includes_packet(self) -> bool {
        matches!(self, Self::Packet | Self::All)
    }
}

fn eval_metadirector_payload(raw: &[String]) -> Result<serde_json::Value, String> {
    let args = parse_eval_metadirector_args(raw)?;
    let text = std::fs::read_to_string(&args.fixtures_path).map_err(|err| {
        format!(
            "Error reading eval fixtures {}: {err}",
            args.fixtures_path.display()
        )
    })?;
    let fixtures: EvalFixtureFile =
        serde_json::from_str(&text).map_err(|err| format!("Error parsing eval fixtures: {err}"))?;
    if fixtures.fixtures.is_empty() {
        return Err("Error: eval fixture file contains no fixtures".to_string());
    }

    let mut rows = Vec::new();
    let mut packet_rows = Vec::new();
    let mut classification_hits = 0usize;
    let mut escalation_hits = 0usize;
    let mut escalation_expected = 0usize;
    let mut escalation_recalled = 0usize;
    let mut escalation_false_positive = 0usize;
    let mut packet_within_budget = 0usize;
    let mut packet_quality_checks_covered = 0usize;
    let mut packet_total_quality_checks = 0usize;
    let mut packet_escalation_hits = 0usize;
    let mut packet_token_total = 0u64;
    let mut packet_token_max = 0u64;

    for fixture in fixtures.fixtures.iter() {
        let mut recommend_raw = vec![
            "--classifier".to_string(),
            args.classifier_mode.as_str().to_string(),
            "--classifier-threshold".to_string(),
            args.classifier_threshold.to_string(),
        ];
        if let Some(endpoint) = args.mlx_endpoint.as_ref() {
            recommend_raw.push("--mlx-endpoint".to_string());
            recommend_raw.push(endpoint.clone());
        }
        recommend_raw.push(fixture.task.clone());
        let recommendation = recommend_payload(&recommend_raw)?;
        let task_type = recommendation
            .pointer("/classification/task_type")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown")
            .to_string();
        let confidence = recommendation
            .pointer("/classification/confidence")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let classification_match = task_type == fixture.expected_classification;
        let escalation_recommended = should_escalate(&fixture.task, &task_type, confidence);

        if args.arm.includes_classifier() {
            if classification_match {
                classification_hits += 1;
            }

            if fixture.escalation_expected {
                escalation_expected += 1;
                if escalation_recommended {
                    escalation_recalled += 1;
                }
            } else if escalation_recommended {
                escalation_false_positive += 1;
            }
            let escalation_match = escalation_recommended == fixture.escalation_expected;
            if escalation_match {
                escalation_hits += 1;
            }

            rows.push(serde_json::json!({
                "id": fixture.id,
                "expected_classification": fixture.expected_classification,
                "actual_classification": task_type,
                "classification_match": classification_match,
                "confidence": confidence,
                "escalation_expected": fixture.escalation_expected,
                "escalation_recommended": escalation_recommended,
                "escalation_match": escalation_match,
                "manager": recommendation.get("manager").cloned().unwrap_or(serde_json::Value::Null),
                "participants": recommendation.get("participants").cloned().unwrap_or(serde_json::Value::Null),
                "classifier": recommendation.pointer("/classification/classifier").cloned().unwrap_or(serde_json::Value::Null),
            }));
        }

        if args.arm.includes_packet() {
            let packet_eval = packet_eval_for_fixture(
                fixture,
                &task_type,
                confidence,
                classification_match,
                escalation_recommended,
                args.packet_budget_tokens,
            );
            if packet_eval.within_budget {
                packet_within_budget += 1;
            }
            packet_quality_checks_covered += packet_eval.quality_checks_covered;
            packet_total_quality_checks += fixture.quality_checks.len();
            if packet_eval.escalation_match {
                packet_escalation_hits += 1;
            }
            packet_token_total = packet_token_total.saturating_add(packet_eval.token_estimate);
            packet_token_max = packet_token_max.max(packet_eval.token_estimate);
            packet_rows.push(packet_eval.to_json());
        }
    }

    let total = fixtures.fixtures.len();
    let classifier_denominator = if args.arm.includes_classifier() {
        total
    } else {
        0
    };
    let packet_denominator = if args.arm.includes_packet() { total } else { 0 };
    let classification_accuracy = ratio(classification_hits, classifier_denominator);
    let escalation_accuracy = ratio(escalation_hits, classifier_denominator);
    let escalation_recall = if escalation_expected == 0 {
        1.0
    } else {
        ratio(escalation_recalled, escalation_expected)
    };
    let packet_budget_pass_rate = ratio(packet_within_budget, packet_denominator);
    let packet_check_coverage = ratio(packet_quality_checks_covered, packet_total_quality_checks);
    let packet_escalation_accuracy = ratio(packet_escalation_hits, packet_denominator);
    // Working-set source for the context gate: an explicit `--ledger` snapshot
    // (the real concurrently-active tasks) wins; otherwise fall back to the
    // fixture packet sum. The ledger path is a *pinned* file, so the eval stays
    // deterministic. This is the finding #1 fix — working set from real ledger
    // rows, not fixture-suite size.
    let ledger_snapshot = match args.ledger.as_ref() {
        Some(path) => Some(load_ledger_snapshot(path)?),
        None => None,
    };
    let ledger_active_task_count = ledger_snapshot
        .as_ref()
        .map(|tasks| tasks.iter().filter(|t| t.status.is_active()).count());
    let manager_prompt_source = if ledger_snapshot.is_some() {
        "ledger"
    } else if args.arm.includes_packet() {
        "fixtures"
    } else {
        "none"
    };
    let (manager_prompt_estimated_tokens, full_transcript_proxy_tokens) = if let Some(tasks) =
        ledger_snapshot.as_ref()
    {
        let active = tasks.iter().filter(|t| t.status.is_active()).count() as u64;
        (
            ledger_working_set_tokens(tasks).saturating_add(manager_prompt_overhead_tokens(active)),
            active.saturating_mul(FULL_TRANSCRIPT_PROXY_TOKENS_PER_FIXTURE),
        )
    } else {
        (
            if args.arm.includes_packet() {
                packet_token_total.saturating_add(manager_prompt_overhead_tokens(total as u64))
            } else {
                0
            },
            (total as u64).saturating_mul(FULL_TRANSCRIPT_PROXY_TOKENS_PER_FIXTURE),
        )
    };
    // A pinned ledger always participates in the context gate; without one, the
    // gate only applies when the packet arm runs (preserving prior behavior).
    let manager_prompt_within_limit = if ledger_snapshot.is_some() {
        manager_prompt_estimated_tokens <= args.manager_prompt_limit_tokens
    } else {
        !args.arm.includes_packet()
            || manager_prompt_estimated_tokens <= args.manager_prompt_limit_tokens
    };
    let context_reduction_ratio = ratio_u64(
        full_transcript_proxy_tokens.saturating_sub(manager_prompt_estimated_tokens),
        full_transcript_proxy_tokens,
    );
    let cost_estimate = cost_estimate_payload(
        args.rate_file.as_ref(),
        manager_prompt_estimated_tokens,
        full_transcript_proxy_tokens,
    )?;
    let classifier_ready = !args.arm.includes_classifier()
        || (classification_accuracy >= 0.80
            && escalation_recall >= 0.95
            && escalation_false_positive <= total.saturating_div(3));
    let packet_ready = !args.arm.includes_packet()
        || (packet_budget_pass_rate >= 1.0
            && packet_check_coverage >= 1.0
            && packet_escalation_accuracy >= 0.95);
    let context_ready = if ledger_snapshot.is_some() {
        manager_prompt_within_limit
    } else {
        !args.arm.includes_packet() || manager_prompt_within_limit
    };
    let thin_ready = classifier_ready && packet_ready && context_ready;
    let scorecard = serde_json::json!({
        "schema": "agent-swarm/metadirector-scorecard/v1",
        "routing_gate": classifier_ready,
        "packet_gate": packet_ready,
        "context_gate": context_ready,
        "thin_default_ready": thin_ready,
        "manager_prompt_source": manager_prompt_source,
        "manager_prompt_estimated_tokens": manager_prompt_estimated_tokens,
        "manager_prompt_limit_tokens": args.manager_prompt_limit_tokens,
        "packet_token_total": packet_token_total,
        "packet_token_max": packet_token_max,
        "full_transcript_proxy_tokens": full_transcript_proxy_tokens,
        "context_reduction_ratio": context_reduction_ratio,
    });

    let mut payload = serde_json::json!({
        "schema": "agent-swarm/metadirector-eval/v1",
        "run_id": new_eval_run_id(),
        "fixtures_path": args.fixtures_path.display().to_string(),
        "arm": args.arm.as_str(),
        "classifier_mode": args.classifier_mode.as_str(),
        "classifier_threshold": args.classifier_threshold,
        "mlx_endpoint": args.mlx_endpoint,
        "packet_budget_tokens": args.packet_budget_tokens,
        "manager_prompt_limit_tokens": args.manager_prompt_limit_tokens,
        "manager_prompt_estimated_tokens": manager_prompt_estimated_tokens,
        "manager_prompt_within_limit": manager_prompt_within_limit,
        "manager_prompt_source": manager_prompt_source,
        "ledger_path": args.ledger.as_ref().map(|path| path.display().to_string()),
        "ledger_active_task_count": ledger_active_task_count,
        "full_transcript_proxy_tokens": full_transcript_proxy_tokens,
        "full_transcript_proxy_tokens_per_fixture": FULL_TRANSCRIPT_PROXY_TOKENS_PER_FIXTURE,
        "context_reduction_ratio": context_reduction_ratio,
        "cost_estimate": cost_estimate,
        "fixture_count": total,
        "classification_accuracy": classification_accuracy,
        "classification_hits": classification_hits,
        "escalation_accuracy": escalation_accuracy,
        "escalation_recall": escalation_recall,
        "escalation_expected_count": escalation_expected,
        "escalation_recalled_count": escalation_recalled,
        "escalation_false_positive_count": escalation_false_positive,
        "packet_budget_pass_rate": packet_budget_pass_rate,
        "packet_checks_covered": packet_quality_checks_covered,
        "packet_checks_total": packet_total_quality_checks,
        "packet_check_coverage": packet_check_coverage,
        "packet_escalation_accuracy": packet_escalation_accuracy,
        "packet_token_total": packet_token_total,
        "packet_token_max": packet_token_max,
        "thin_default_ready": thin_ready,
        "verdict": if thin_ready {
            "routing gate passed: thin defaults are acceptable for this fixture set with escalation enabled"
        } else {
            "routing gate failed: keep larger-manager defaults or improve routing/escalation heuristics before slimming"
        },
        "limitations": [
            "This eval scores classifier routing and deterministic packet readiness; it does not yet run live A/B synthesis quality.",
            "Gemma MLX classifier availability is reported per row; when unavailable, routing falls back to deterministic Rust classification."
        ],
        "scorecard": scorecard,
        "rows": rows,
        "packet_rows": packet_rows,
    });

    if args.write_summary {
        let path = write_eval_summary(&payload)?;
        let scorecard_path = write_eval_scorecard(&payload)?;
        payload["summary_path"] = serde_json::json!(path.display().to_string());
        payload["scorecard_path"] = serde_json::json!(scorecard_path.display().to_string());
    }
    Ok(payload)
}

struct EvalArgs {
    fixtures_path: PathBuf,
    arm: EvalArm,
    classifier_mode: ClassifierMode,
    classifier_threshold: u8,
    mlx_endpoint: Option<String>,
    packet_budget_tokens: u64,
    manager_prompt_limit_tokens: u64,
    rate_file: Option<PathBuf>,
    ledger: Option<PathBuf>,
    write_summary: bool,
}

fn parse_eval_metadirector_args(raw: &[String]) -> Result<EvalArgs, String> {
    let mut fixtures_path: Option<PathBuf> = None;
    let mut arm = EvalArm::Classifier;
    let mut classifier_mode = ClassifierMode::default();
    let mut classifier_threshold = 80u8;
    let mut mlx_endpoint: Option<String> = None;
    let mut packet_budget_tokens = 300u64;
    let mut manager_prompt_limit_tokens = DEFAULT_MANAGER_PROMPT_LIMIT_TOKENS;
    let mut rate_file: Option<PathBuf> = None;
    let mut ledger: Option<PathBuf> = None;
    let mut write_summary = true;
    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--arm" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --arm requires a value".to_string())?;
                arm = EvalArm::from_str(value)?;
            }
            "--fixtures" | "--fixture" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("Error: {arg} requires a path"))?;
                fixtures_path = Some(PathBuf::from(value));
            }
            "--classifier" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --classifier requires a value".to_string())?;
                classifier_mode = ClassifierMode::from_str(value)?;
            }
            "--classifier-threshold" | "--mlx-threshold" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("Error: {arg} requires a value"))?;
                classifier_threshold = parse_classifier_threshold(value)?;
            }
            "--mlx-endpoint" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Error: --mlx-endpoint requires a value".to_string())?;
                mlx_endpoint = Some(value.clone());
            }
            "--packet-budget" | "--packet-budget-tokens" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("Error: {arg} requires a value"))?;
                packet_budget_tokens = parse_u64_range(value, "packet budget", 1, 2000)?;
            }
            "--manager-prompt-limit" | "--manager-prompt-limit-tokens" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("Error: {arg} requires a value"))?;
                manager_prompt_limit_tokens =
                    parse_u64_range(value, "manager prompt limit", 1, 1_000_000)?;
            }
            "--rate-file" | "--rates" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("Error: {arg} requires a path"))?;
                rate_file = Some(PathBuf::from(value));
            }
            "--ledger" | "--ledger-file" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("Error: {arg} requires a path"))?;
                ledger = Some(PathBuf::from(value));
            }
            "--write-summary" => write_summary = true,
            "--no-write-summary" => write_summary = false,
            "--help" | "-h" => {
                return Err(
                    "usage: agent-swarm eval-metadirector [--arm classifier|packet|all] [--fixtures PATH] [--classifier auto|deterministic|semantic] [--classifier-threshold N] [--mlx-endpoint URL] [--packet-budget N] [--manager-prompt-limit N] [--rate-file PATH] [--ledger PATH] [--no-write-summary]"
                        .to_string(),
                );
            }
            _ if arg.starts_with("--arm=") => {
                arm = EvalArm::from_str(&arg["--arm=".len()..])?;
            }
            _ if arg.starts_with("--classifier=") => {
                classifier_mode = ClassifierMode::from_str(&arg["--classifier=".len()..])?;
            }
            _ if arg.starts_with("--classifier-threshold=") => {
                classifier_threshold =
                    parse_classifier_threshold(&arg["--classifier-threshold=".len()..])?;
            }
            _ if arg.starts_with("--mlx-threshold=") => {
                classifier_threshold =
                    parse_classifier_threshold(&arg["--mlx-threshold=".len()..])?;
            }
            _ if arg.starts_with("--mlx-endpoint=") => {
                mlx_endpoint = Some(arg["--mlx-endpoint=".len()..].to_string());
            }
            _ if arg.starts_with("--packet-budget=") => {
                packet_budget_tokens =
                    parse_u64_range(&arg["--packet-budget=".len()..], "packet budget", 1, 2000)?;
            }
            _ if arg.starts_with("--packet-budget-tokens=") => {
                packet_budget_tokens = parse_u64_range(
                    &arg["--packet-budget-tokens=".len()..],
                    "packet budget",
                    1,
                    2000,
                )?;
            }
            _ if arg.starts_with("--manager-prompt-limit=") => {
                manager_prompt_limit_tokens = parse_u64_range(
                    &arg["--manager-prompt-limit=".len()..],
                    "manager prompt limit",
                    1,
                    1_000_000,
                )?;
            }
            _ if arg.starts_with("--manager-prompt-limit-tokens=") => {
                manager_prompt_limit_tokens = parse_u64_range(
                    &arg["--manager-prompt-limit-tokens=".len()..],
                    "manager prompt limit",
                    1,
                    1_000_000,
                )?;
            }
            _ if arg.starts_with("--rate-file=") => {
                rate_file = Some(PathBuf::from(&arg["--rate-file=".len()..]));
            }
            _ if arg.starts_with("--rates=") => {
                rate_file = Some(PathBuf::from(&arg["--rates=".len()..]));
            }
            _ if arg.starts_with("--ledger=") => {
                ledger = Some(PathBuf::from(&arg["--ledger=".len()..]));
            }
            _ if arg.starts_with("--ledger-file=") => {
                ledger = Some(PathBuf::from(&arg["--ledger-file=".len()..]));
            }
            other => return Err(format!("Error: unknown eval-metadirector option `{other}`")),
        }
    }

    Ok(EvalArgs {
        fixtures_path: fixtures_path.unwrap_or_else(default_eval_fixtures_path),
        arm,
        classifier_mode,
        classifier_threshold,
        mlx_endpoint,
        packet_budget_tokens,
        manager_prompt_limit_tokens,
        rate_file,
        ledger,
        write_summary,
    })
}

fn default_eval_fixtures_path() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let repo_path = cwd.join("docs/agents/evals/metadirector/fixtures.json");
    if Path::new(&repo_path).exists() {
        repo_path
    } else {
        PathBuf::from("docs/agents/evals/metadirector/fixtures.json")
    }
}

#[derive(Debug)]
struct PacketEval {
    fixture_id: String,
    packet: serde_json::Value,
    token_estimate: u64,
    within_budget: bool,
    quality_checks_covered: usize,
    escalation_match: bool,
}

impl PacketEval {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.fixture_id,
            "packet": self.packet,
            "token_estimate": self.token_estimate,
            "within_budget": self.within_budget,
            "quality_checks_covered": self.quality_checks_covered,
            "escalation_match": self.escalation_match,
        })
    }
}

fn packet_eval_for_fixture(
    fixture: &EvalFixture,
    task_type: &str,
    confidence: u64,
    classification_match: bool,
    escalation_recommended: bool,
    packet_budget_tokens: u64,
) -> PacketEval {
    let constraints = bounded_strings(
        fixture
            .must_touch
            .iter()
            .map(|path| format!("touch:{path}"))
            .chain(
                fixture
                    .must_not_touch
                    .iter()
                    .map(|path| format!("protect:{path}")),
            ),
        6,
        72,
    );
    let tests = bounded_strings(fixture.quality_checks.iter().cloned(), 5, 72);
    let blockers = if escalation_recommended {
        vec![format!("escalate: class={task_type} conf={confidence}")]
    } else {
        Vec::new()
    };
    let risks = bounded_strings(
        [
            (!classification_match).then(|| {
                format!(
                    "class mismatch: expected {}, got {}",
                    fixture.expected_classification, task_type
                )
            }),
            (!fixture.must_not_touch.is_empty()).then(|| "protected scope present".to_string()),
            (confidence < 80).then(|| "low classifier confidence".to_string()),
        ]
        .into_iter()
        .flatten(),
        3,
        80,
    );
    let packet = serde_json::json!({
        "schema": "agent-swarm/handoff-packet/v1",
        "task_id": fixture.id,
        "worker_id": format!("eval-packet/{task_type}"),
        "confidence": ((confidence as f64 / 100.0) * 100.0).round() / 100.0,
        "findings": bounded_strings([
            format!("class={task_type}; expected={}", fixture.expected_classification),
            format!("task={}", truncate_chars(&fixture.task, 120)),
        ], 4, 150),
        "risks": risks,
        "blockers": blockers,
        "steps": bounded_strings([
            "respect scopes".to_string(),
            "make smallest change".to_string(),
            "run fixture checks".to_string(),
        ], 5, 80),
        "tests": tests,
        "citations": [format!("fixtures.json#{}", fixture.id)],
        "constraints": constraints,
        "flags": if classification_match { Vec::<String>::new() } else { vec!["CONFLICT".to_string()] },
    });
    let token_estimate = estimate_tokens_json(&packet);
    PacketEval {
        fixture_id: fixture.id.clone(),
        packet,
        token_estimate,
        within_budget: token_estimate <= packet_budget_tokens,
        quality_checks_covered: fixture.quality_checks.len(),
        escalation_match: escalation_recommended == fixture.escalation_expected,
    }
}

fn bounded_strings<I>(items: I, limit: usize, max_chars: usize) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    items
        .into_iter()
        .take(limit)
        .map(|item| truncate_chars(&item, max_chars))
        .collect()
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        format!(
            "{}...",
            value
                .chars()
                .take(max_chars.saturating_sub(3))
                .collect::<String>()
        )
    }
}

fn estimate_tokens_json(value: &serde_json::Value) -> u64 {
    serde_json::to_string(value)
        .map(|text| estimate_tokens_text(&text))
        .unwrap_or(0)
}

fn estimate_tokens_text(value: &str) -> u64 {
    value.chars().count().div_ceil(4) as u64
}

fn manager_prompt_overhead_tokens(fixture_count: u64) -> u64 {
    120u64.saturating_add(fixture_count.saturating_mul(16))
}

fn ratio_u64(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        ((numerator as f64 / denominator as f64) * 1000.0).round() / 1000.0
    }
}

fn cost_estimate_payload(
    rate_file: Option<&PathBuf>,
    manager_prompt_tokens: u64,
    full_transcript_proxy_tokens: u64,
) -> Result<serde_json::Value, String> {
    let Some(path) = rate_file else {
        return Ok(serde_json::json!({
            "rate_file": serde_json::Value::Null,
            "currency": serde_json::Value::Null,
            "thin_manager_input_usd_per_1k": serde_json::Value::Null,
            "large_manager_input_usd_per_1k": serde_json::Value::Null,
            "thin_manager_input_cost": serde_json::Value::Null,
            "large_manager_input_cost_proxy": serde_json::Value::Null,
            "savings_proxy": serde_json::Value::Null,
        }));
    };
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("Error reading rate file {}: {err}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|err| format!("Error parsing rate file {}: {err}", path.display()))?;
    let currency = value
        .get("currency")
        .and_then(|item| item.as_str())
        .unwrap_or("USD");
    let thin_rate = rate_value(&value, "thin_manager_input_usd_per_1k")
        .or_else(|| model_rate_value(&value, "thin_manager"));
    let large_rate = rate_value(&value, "large_manager_input_usd_per_1k")
        .or_else(|| model_rate_value(&value, "large_manager"));
    let thin_cost = thin_rate.map(|rate| cost_for_tokens(manager_prompt_tokens, rate));
    let large_cost = large_rate.map(|rate| cost_for_tokens(full_transcript_proxy_tokens, rate));
    let savings = match (thin_cost, large_cost) {
        (Some(thin), Some(large)) => Some(round_money((large - thin).max(0.0))),
        _ => None,
    };
    Ok(serde_json::json!({
        "rate_file": path.display().to_string(),
        "currency": currency,
        "thin_manager_input_usd_per_1k": thin_rate,
        "large_manager_input_usd_per_1k": large_rate,
        "thin_manager_input_cost": thin_cost,
        "large_manager_input_cost_proxy": large_cost,
        "savings_proxy": savings,
    }))
}

fn rate_value(value: &serde_json::Value, key: &str) -> Option<f64> {
    value.get(key).and_then(|item| item.as_f64())
}

fn model_rate_value(value: &serde_json::Value, model_key: &str) -> Option<f64> {
    value
        .pointer(&format!("/models/{model_key}/input_usd_per_1k"))
        .or_else(|| value.pointer(&format!("/models/{model_key}/input_per_1k_usd")))
        .or_else(|| value.pointer(&format!("/models/{model_key}/input_per_1k")))
        .and_then(|item| item.as_f64())
}

fn cost_for_tokens(tokens: u64, usd_per_1k: f64) -> f64 {
    round_money((tokens as f64 / 1000.0) * usd_per_1k)
}

fn round_money(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn parse_u64_range(value: &str, label: &str, min: u64, max: u64) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("Error: {label} must be an integer"))?;
    if parsed < min || parsed > max {
        Err(format!("Error: {label} must be between {min} and {max}"))
    } else {
        Ok(parsed)
    }
}

fn new_eval_run_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("eval-{millis}")
}

fn write_eval_summary(payload: &serde_json::Value) -> Result<PathBuf, String> {
    let run_id = payload
        .get("run_id")
        .and_then(|value| value.as_str())
        .unwrap_or("eval-unknown");
    let base = swarm_store::store::swarm_home()
        .ok_or_else(|| {
            "Error: cannot resolve the swarm home (set SWARM_HOME or HOME); \
             cannot write eval summary"
                .to_string()
        })?
        .join("evals")
        .join(run_id);
    std::fs::create_dir_all(&base)
        .map_err(|err| format!("Error creating eval summary dir {}: {err}", base.display()))?;
    let path = base.join("summary.json");
    let mut summary = payload.clone();
    summary["summary_path"] = serde_json::json!(path.display().to_string());
    let text = serde_json::to_string_pretty(&summary)
        .map_err(|err| format!("Error serializing eval summary: {err}"))?;
    std::fs::write(&path, text)
        .map_err(|err| format!("Error writing eval summary {}: {err}", path.display()))?;
    Ok(path)
}

fn write_eval_scorecard(payload: &serde_json::Value) -> Result<PathBuf, String> {
    let run_id = payload
        .get("run_id")
        .and_then(|value| value.as_str())
        .unwrap_or("eval-unknown");
    let base = swarm_store::store::swarm_home()
        .ok_or_else(|| {
            "Error: cannot resolve the swarm home (set SWARM_HOME or HOME); \
             cannot write eval scorecard"
                .to_string()
        })?
        .join("evals")
        .join(run_id);
    std::fs::create_dir_all(&base).map_err(|err| {
        format!(
            "Error creating eval scorecard dir {}: {err}",
            base.display()
        )
    })?;
    let path = base.join("scorecard.md");
    std::fs::write(&path, render_eval_scorecard(payload))
        .map_err(|err| format!("Error writing eval scorecard {}: {err}", path.display()))?;
    Ok(path)
}

fn render_eval_scorecard(payload: &serde_json::Value) -> String {
    let bool_text = |key: &str| {
        if payload
            .get(key)
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            "pass"
        } else {
            "fail"
        }
    };
    let scorecard = payload
        .get("scorecard")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let context_gate = if scorecard
        .get("context_gate")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        "pass"
    } else {
        "fail"
    };
    format!(
        "# Metadirector Eval Scorecard\n\n- Thin default ready: {}\n- Routing accuracy: {}\n- Escalation accuracy: {}\n- Packet budget pass rate: {}\n- Packet check coverage: {}\n- Manager prompt: {} / {} tokens ({})\n- Context reduction proxy: {}\n- Summary: {}\n",
        bool_text("thin_default_ready"),
        payload.get("classification_accuracy").and_then(|value| value.as_f64()).unwrap_or(0.0),
        payload.get("escalation_accuracy").and_then(|value| value.as_f64()).unwrap_or(0.0),
        payload.get("packet_budget_pass_rate").and_then(|value| value.as_f64()).unwrap_or(0.0),
        payload.get("packet_check_coverage").and_then(|value| value.as_f64()).unwrap_or(0.0),
        payload.get("manager_prompt_estimated_tokens").and_then(|value| value.as_u64()).unwrap_or(0),
        payload.get("manager_prompt_limit_tokens").and_then(|value| value.as_u64()).unwrap_or(0),
        context_gate,
        payload.get("context_reduction_ratio").and_then(|value| value.as_f64()).unwrap_or(0.0),
        payload.get("verdict").and_then(|value| value.as_str()).unwrap_or("unknown"),
    )
}

fn should_escalate(task: &str, task_type: &str, confidence: u64) -> bool {
    let normalized = task.to_ascii_lowercase();
    let has_risky_signal = [
        "access request",
        "credential",
        "decrypt",
        "grant",
        "keychain",
        "lease",
        "migration",
        "permission",
        "provider registry",
        "security",
        "signing",
        "vault",
        "cross-runtime",
        "rust swarm",
        "telemetry",
        "contradict",
    ]
    .iter()
    .any(|needle| normalized.contains(needle));
    if has_risky_signal {
        return true;
    }
    if task_type == "model-provider" {
        return true;
    }
    confidence < 80 && task_type != "docs"
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        ((numerator as f64 / denominator as f64) * 1000.0).round() / 1000.0
    }
}

fn proposals_payload() -> serde_json::Value {
    telemetry::proposals_json()
}

#[derive(Debug, PartialEq)]
struct FeedbackArgs {
    session_id: Option<String>,
    role: String,
    agent: String,
    outcome: String,
    note: Option<String>,
}

#[derive(Debug)]
enum FeedbackParse {
    Help,
    Args(FeedbackArgs),
}

fn parse_feedback_args(raw: &[String]) -> Result<FeedbackParse, String> {
    let mut session_id: Option<String> = None;
    let mut role: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut outcome: Option<String> = None;
    let mut note: Option<String> = None;
    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--session" | "--session-id" => {
                session_id = iter.next().cloned();
            }
            "--role" => {
                role = iter.next().cloned();
            }
            "--agent" => {
                agent = iter.next().cloned();
            }
            "--outcome" => {
                outcome = iter.next().cloned();
            }
            "--note" => {
                note = iter.next().cloned();
            }
            "-h" | "--help" => {
                print_help(load_default_timeout());
                return Ok(FeedbackParse::Help);
            }
            other => return Err(format!("Error: unknown feedback option `{other}`")),
        }
    }
    Ok(FeedbackParse::Args(FeedbackArgs {
        session_id,
        role: role.ok_or_else(|| "Error: feedback requires --role ROLE".to_string())?,
        agent: agent.ok_or_else(|| "Error: feedback requires --agent AGENT".to_string())?,
        outcome: outcome
            .ok_or_else(|| "Error: feedback requires --outcome win|loss".to_string())?,
        note,
    }))
}

#[derive(Debug, PartialEq)]
struct ProposeArgs {
    session_id: Option<String>,
    proposed_by: Option<String>,
    tags: Vec<String>,
    title: String,
    body: String,
}

fn parse_propose_args(raw: &[String]) -> Result<ProposeArgs, String> {
    let mut session_id: Option<String> = None;
    let mut proposed_by: Option<String> = None;
    let mut tags = Vec::new();
    let mut title: Option<String> = None;
    let mut body = Vec::new();
    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--session" | "--session-id" => session_id = iter.next().cloned(),
            "--by" | "--proposed-by" => proposed_by = iter.next().cloned(),
            "--tag" => {
                if let Some(tag) = iter.next() {
                    tags.push(tag.clone());
                }
            }
            "--title" => title = iter.next().cloned(),
            value if title.is_none() => title = Some(value.to_string()),
            value => body.push(value.to_string()),
        }
    }
    let title = title.ok_or_else(|| "Error: propose requires a title".to_string())?;
    let body = if body.is_empty() {
        title.clone()
    } else {
        body.join(" ")
    };
    Ok(ProposeArgs {
        session_id,
        proposed_by,
        tags,
        title,
        body,
    })
}

#[derive(Debug, PartialEq)]
struct ProposalVoteArgs {
    proposal_id: String,
    voter: String,
    vote: String,
    rationale: Option<String>,
}

fn parse_proposal_vote_args(raw: &[String]) -> Result<ProposalVoteArgs, String> {
    let mut proposal_id: Option<String> = None;
    let mut voter: Option<String> = None;
    let mut vote: Option<String> = None;
    let mut rationale = Vec::new();
    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--proposal" | "--proposal-id" => proposal_id = iter.next().cloned(),
            "--voter" | "--by" => voter = iter.next().cloned(),
            "--vote" => vote = iter.next().cloned(),
            value if proposal_id.is_none() => proposal_id = Some(value.to_string()),
            value if vote.is_none() => vote = Some(value.to_string()),
            value if voter.is_none() => voter = Some(value.to_string()),
            value => rationale.push(value.to_string()),
        }
    }
    let proposal_id =
        proposal_id.ok_or_else(|| "Error: proposal-vote requires a proposal id".to_string())?;
    let vote = vote.ok_or_else(|| "Error: proposal-vote requires a vote".to_string())?;
    Ok(ProposalVoteArgs {
        proposal_id,
        voter: voter.unwrap_or_else(|| "user".to_string()),
        vote,
        rationale: if rationale.is_empty() {
            None
        } else {
            Some(rationale.join(" "))
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insights_payload_has_schema() {
        assert_eq!(
            insights_payload()
                .get("schema")
                .and_then(|value| value.as_str()),
            Some("agent-swarm/insights/v1")
        );
    }

    #[test]
    fn profiles_payload_has_schema() {
        assert_eq!(
            profiles_payload()
                .get("schema")
                .and_then(|value| value.as_str()),
            Some("agent-swarm/profiles/v1")
        );
    }

    #[test]
    fn automation_hooks_payload_has_schema() {
        assert_eq!(
            automation_hooks_payload()
                .get("schema")
                .and_then(|value| value.as_str()),
            Some("agent-swarm/automation-hooks/v1")
        );
    }

    #[test]
    fn presets_payload_has_schema() {
        assert_eq!(
            presets_payload()
                .get("schema")
                .and_then(|value| value.as_str()),
            Some("agent-swarm/presets/v1")
        );
    }

    #[test]
    fn recommend_payload_requires_task_and_returns_schema() {
        let err = recommend_payload(&[]).unwrap_err();
        assert!(err.contains("recommend requires a task description"));

        let payload = recommend_payload(&["audit".to_string(), "runtime".to_string()]).unwrap();
        assert_eq!(
            payload.get("schema").and_then(|value| value.as_str()),
            Some("agent-swarm/recommendation/v1")
        );
        assert_eq!(
            payload
                .pointer("/classification/task_type")
                .and_then(|value| value.as_str()),
            Some("audit")
        );
        assert_eq!(
            payload
                .pointer("/classification/classifier/model")
                .and_then(|value| value.as_str()),
            Some("mlx-community/gemma-4-e2b-it-OptiQ-4bit")
        );
        assert_eq!(
            payload
                .pointer("/classification/classifier/status")
                .and_then(|value| value.as_str()),
            Some("deterministic-high-confidence")
        );
        assert_eq!(
            payload
                .pointer("/classification/classifier/invoked")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn recommend_payload_supports_deterministic_classifier_flag() {
        let payload = recommend_payload(&[
            "--classifier".to_string(),
            "deterministic".to_string(),
            "implement".to_string(),
            "the".to_string(),
            "next".to_string(),
            "slice".to_string(),
        ])
        .unwrap();

        assert_eq!(
            payload
                .pointer("/classification/classifier/mode")
                .and_then(|value| value.as_str()),
            Some("deterministic-rust-fallback")
        );
        assert_eq!(
            payload
                .pointer("/classification/classifier/status")
                .and_then(|value| value.as_str()),
            Some("deterministic-rust-fallback")
        );
    }

    #[test]
    fn semantic_classifier_response_parses_json_inside_text() {
        let parsed = parse_semantic_classifier_response(
            "```json\n{\"task_type\":\"ui-design\",\"confidence\":91,\"reason\":\"graph layout\"}\n```",
        )
        .unwrap();

        assert_eq!(parsed.task_type, "ui-design");
        assert_eq!(parsed.confidence, 91);
        assert_eq!(parsed.reason.as_deref(), Some("graph layout"));
    }

    #[test]
    fn eval_args_support_classifier_mode() {
        let parsed = parse_eval_metadirector_args(&[
            "--classifier=deterministic".to_string(),
            "--classifier-threshold".to_string(),
            "75".to_string(),
            "--mlx-endpoint=http://127.0.0.1:8081".to_string(),
            "--arm".to_string(),
            "packet".to_string(),
            "--packet-budget=250".to_string(),
            "--manager-prompt-limit".to_string(),
            "1500".to_string(),
            "--rate-file=rates.json".to_string(),
            "--no-write-summary".to_string(),
            "--fixtures".to_string(),
            "fixtures.json".to_string(),
        ])
        .unwrap();

        assert_eq!(parsed.arm, EvalArm::Packet);
        assert_eq!(parsed.classifier_mode, ClassifierMode::Deterministic);
        assert_eq!(parsed.classifier_threshold, 75);
        assert_eq!(
            parsed.mlx_endpoint.as_deref(),
            Some("http://127.0.0.1:8081")
        );
        assert_eq!(parsed.packet_budget_tokens, 250);
        assert_eq!(parsed.manager_prompt_limit_tokens, 1500);
        assert_eq!(parsed.rate_file, Some(PathBuf::from("rates.json")));
        assert!(!parsed.write_summary);
        assert_eq!(parsed.fixtures_path, PathBuf::from("fixtures.json"));
    }

    #[test]
    fn packet_eval_builds_bounded_packet_with_checks() {
        let fixture = EvalFixture {
            id: "fixture-1".to_string(),
            task: "Fix graph wires and node expansion".to_string(),
            expected_classification: "ui-design".to_string(),
            must_touch: vec!["packages/panels/graph_panel".to_string()],
            must_not_touch: vec!["mesh/crates/secret-vault".to_string()],
            quality_checks: vec![
                "Wire endpoints stay attached.".to_string(),
                "Collapsed nodes hide expanded-only details.".to_string(),
            ],
            escalation_expected: false,
        };

        let packet = packet_eval_for_fixture(&fixture, "ui-design", 86, true, false, 300);

        assert!(packet.within_budget);
        assert_eq!(packet.quality_checks_covered, 2);
        assert!(packet.escalation_match);
        assert_eq!(
            packet.packet.get("schema").and_then(|value| value.as_str()),
            Some("agent-swarm/handoff-packet/v1")
        );
        assert_eq!(
            packet
                .packet
                .get("tests")
                .and_then(|value| value.as_array())
                .map(Vec::len),
            Some(2)
        );
    }

    #[test]
    fn eval_packet_arm_emits_packet_rows_without_summary_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let fixtures = dir.path().join("fixtures.json");
        std::fs::write(
            &fixtures,
            serde_json::json!({
                "schema": "swarm/metadirector-eval-fixtures/v1",
                "fixtures": [{
                    "id": "docs-1",
                    "task": "Update README docs for install verification",
                    "expected_classification": "docs",
                    "must_touch": ["README.md"],
                    "must_not_touch": ["secrets"],
                    "quality_checks": ["Verification command is documented."],
                    "escalation_expected": false
                }]
            })
            .to_string(),
        )
        .unwrap();

        let payload = eval_metadirector_payload(&[
            "--arm=packet".to_string(),
            "--classifier=deterministic".to_string(),
            "--no-write-summary".to_string(),
            "--fixtures".to_string(),
            fixtures.display().to_string(),
        ])
        .unwrap();

        assert_eq!(payload["arm"], "packet");
        assert!(payload.get("summary_path").is_none());
        assert_eq!(payload["packet_rows"].as_array().unwrap().len(), 1);
        assert_eq!(payload["packet_check_coverage"], 1.0);
        assert!(payload["manager_prompt_estimated_tokens"].as_u64().unwrap() > 0);
        assert_eq!(payload["manager_prompt_within_limit"], true);
        assert!(payload["scorecard"].is_object());
    }

    #[test]
    fn eval_rate_file_estimates_manager_cost_proxy() {
        let dir = tempfile::tempdir().unwrap();
        let fixtures = dir.path().join("fixtures.json");
        let rates = dir.path().join("rates.json");
        std::fs::write(
            &fixtures,
            serde_json::json!({
                "schema": "swarm/metadirector-eval-fixtures/v1",
                "fixtures": [{
                    "id": "ui-1",
                    "task": "Fix graph node layout",
                    "expected_classification": "ui-design",
                    "must_touch": ["packages/panels/graph_panel"],
                    "must_not_touch": ["mesh/crates/secret-vault"],
                    "quality_checks": ["Graph layout is stable."],
                    "escalation_expected": false
                }]
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            &rates,
            serde_json::json!({
                "currency": "USD",
                "models": {
                    "thin_manager": {"input_per_1k": 0.10},
                    "large_manager": {"input_per_1k": 1.00}
                }
            })
            .to_string(),
        )
        .unwrap();

        let payload = eval_metadirector_payload(&[
            "--arm=all".to_string(),
            "--classifier=deterministic".to_string(),
            "--packet-budget=300".to_string(),
            "--manager-prompt-limit=2000".to_string(),
            "--rate-file".to_string(),
            rates.display().to_string(),
            "--no-write-summary".to_string(),
            "--fixtures".to_string(),
            fixtures.display().to_string(),
        ])
        .unwrap();

        assert_eq!(payload["thin_default_ready"], true);
        assert_eq!(payload["cost_estimate"]["currency"], "USD");
        assert!(
            payload["cost_estimate"]["thin_manager_input_cost"]
                .as_f64()
                .unwrap()
                > 0.0
        );
        assert!(
            payload["cost_estimate"]["large_manager_input_cost_proxy"]
                .as_f64()
                .unwrap()
                > payload["cost_estimate"]["thin_manager_input_cost"]
                    .as_f64()
                    .unwrap()
        );
    }

    #[test]
    fn metadirector_escalation_heuristic_catches_risky_work() {
        assert!(should_escalate(
            "harden vault access request grants",
            "audit",
            82
        ));
        assert!(should_escalate(
            "wire DeepSeek provider registry access",
            "model-provider",
            84
        ));
        assert!(should_escalate(
            "audit cross-runtime Flutter and Rust telemetry boundary",
            "ui-design",
            86
        ));
        assert!(!should_escalate(
            "Update install verification docs for signed shim help behavior",
            "docs",
            78
        ));
        assert!(!should_escalate(
            "Design deterministic worker handoff packets that reduce metadirector context",
            "audit",
            82
        ));
        assert!(!should_escalate(
            "fix graph node hover animation",
            "ui-design",
            86
        ));
    }

    #[test]
    fn feedback_args_require_core_fields_and_parse_values() {
        let err = parse_feedback_args(&["--agent".to_string(), "gemini".to_string()]).unwrap_err();
        assert!(err.contains("feedback requires --role ROLE"));

        let parsed = parse_feedback_args(&[
            "--session".to_string(),
            "session-1".to_string(),
            "--role".to_string(),
            "qa".to_string(),
            "--agent".to_string(),
            "claude:sonnet".to_string(),
            "--outcome".to_string(),
            "win".to_string(),
            "--note".to_string(),
            "caught risk".to_string(),
        ])
        .unwrap();
        match parsed {
            FeedbackParse::Args(args) => assert_eq!(
                args,
                FeedbackArgs {
                    session_id: Some("session-1".to_string()),
                    role: "qa".to_string(),
                    agent: "claude:sonnet".to_string(),
                    outcome: "win".to_string(),
                    note: Some("caught risk".to_string()),
                }
            ),
            FeedbackParse::Help => panic!("expected parsed feedback args"),
        }
    }

    #[test]
    fn feedback_args_support_help() {
        let parsed = parse_feedback_args(&["--help".to_string()]).unwrap();

        match parsed {
            FeedbackParse::Help => {}
            FeedbackParse::Args(_) => panic!("expected help parse result"),
        }
    }

    #[test]
    fn proposals_payload_has_schema() {
        assert_eq!(
            proposals_payload()
                .get("schema")
                .and_then(|value| value.as_str()),
            Some("agent-swarm/proposals/v1")
        );
    }

    #[test]
    fn propose_args_require_title_and_default_body_to_title() {
        let err = parse_propose_args(&[]).unwrap_err();
        assert!(err.contains("propose requires a title"));

        let parsed = parse_propose_args(&[
            "--session".to_string(),
            "session-1".to_string(),
            "--by".to_string(),
            "manager".to_string(),
            "--tag".to_string(),
            "phase1".to_string(),
            "--title".to_string(),
            "Improve telemetry".to_string(),
        ])
        .unwrap();
        assert_eq!(parsed.body, "Improve telemetry");
        assert_eq!(parsed.tags, vec!["phase1"]);
        assert_eq!(parsed.proposed_by, Some("manager".to_string()));
    }

    #[test]
    fn proposal_vote_args_require_id_and_vote_with_default_voter() {
        let err = parse_proposal_vote_args(&["proposal-1".to_string()]).unwrap_err();
        assert!(err.contains("proposal-vote requires a vote"));

        let parsed = parse_proposal_vote_args(&[
            "proposal-1".to_string(),
            "approve".to_string(),
            "looks".to_string(),
            "good".to_string(),
        ])
        .unwrap();
        assert_eq!(
            parsed,
            ProposalVoteArgs {
                proposal_id: "proposal-1".to_string(),
                voter: "looks".to_string(),
                vote: "approve".to_string(),
                rationale: Some("good".to_string()),
            }
        );
    }
}
