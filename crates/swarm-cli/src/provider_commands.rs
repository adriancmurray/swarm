//! `swarm provider ...` — manage stored provider configurations and their
//! credentials.
//!
//! The management surface over `swarm-manager`'s `ProviderRegistry`: the same
//! registry the `native` backend resolves at run time (rooted at
//! `swarm_home()/providers`). Key material is never accepted as an argv token
//! (ps/shell-history leakage), never echoed, and never logged — `key set`
//! reads from stdin or copies from a named environment variable, and the only
//! acknowledgment is a masked length.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use swarm_manager::{KeyStatus, ProviderConfig, ProviderRegistry, ProviderType};
use swarm_store::store::{providers_dir, swarm_home_err};

/// Provider-type tokens accepted by `--type`, mirrored from
/// `ProviderType::from_str` (everything except the `none` sentinel).
const VALID_PROVIDER_TYPES: &[&str] = &[
    "anthropic",
    "deepseek",
    "gemini",
    "lmstudio",
    "mlx",
    "ollama",
    "openai",
    "openrouter",
];

const PROVIDER_USAGE: &str = "usage: swarm provider add ID --type TYPE [--name NAME] [--endpoint URL] [--models A,B,C] [--data-dir PATH]\n\
       swarm provider models [TYPE]\n\
       swarm provider list [--data-dir PATH]\n\
       swarm provider key set ID [--from-env VAR] [--data-dir PATH]   (key is read from stdin, never argv)\n\
       swarm provider key check ID [--data-dir PATH]\n\
       swarm provider remove ID [--data-dir PATH]";

/// Entry point for `swarm provider ...`: resolves the shared data dir
/// (`--data-dir` override wins), opens the registry with the production
/// keychain vault, and dispatches.
pub(crate) fn cmd_provider(raw: &[String]) -> Result<i32, String> {
    let (rest, data_dir) = split_data_dir(raw)?;
    let data_dir = match data_dir {
        Some(dir) => dir,
        None => providers_dir().ok_or_else(swarm_home_err)?,
    };
    let registry = ProviderRegistry::open(&data_dir)
        .map_err(|e| format!("Error opening provider registry: {e}"))?;
    let stdin = io::stdin();
    let mut out = io::stdout();
    run_provider(&registry, &rest, &mut stdin.lock(), &mut out)
}

/// Pull a `--data-dir PATH` pair out of the args (anywhere in the list).
fn split_data_dir(raw: &[String]) -> Result<(Vec<String>, Option<PathBuf>), String> {
    let mut rest = Vec::new();
    let mut data_dir = None;
    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        if arg == "--data-dir" {
            let value = iter
                .next()
                .ok_or_else(|| "Error: --data-dir requires a path.".to_string())?;
            data_dir = Some(PathBuf::from(value));
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((rest, data_dir))
}

/// Dispatch a `provider` subcommand against an already-open registry.
///
/// The test seam: tests construct the registry via
/// `ProviderRegistry::open_with_vault` (keychain-free) over a tempdir, feed
/// `key_input` from a `Cursor`, and capture `out` in a `Vec<u8>`.
pub(crate) fn run_provider(
    registry: &ProviderRegistry,
    args: &[String],
    key_input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<i32, String> {
    match args.first().map(String::as_str) {
        Some("add") => provider_add(registry, &args[1..], out),
        Some("models") => provider_models(&args[1..], out),
        Some("list") => provider_list(registry, out),
        Some("key") => match args.get(1).map(String::as_str) {
            Some("set") => provider_key_set(registry, &args[2..], key_input, out),
            Some("check") => provider_key_check(registry, &args[2..], out),
            _ => Err(format!(
                "Error: unknown `provider key` subcommand. Expected set or check.\n{PROVIDER_USAGE}"
            )),
        },
        Some("remove") => provider_remove(registry, &args[1..], out),
        _ => Err(format!(
            "Error: unknown `provider` subcommand. Expected add, models, list, key, or remove.\n{PROVIDER_USAGE}"
        )),
    }
}

/// Closing line for any model-suggestion output: the lists above are
/// best-effort snapshots, not a source of truth.
pub(crate) const MODEL_CATALOG_NUDGE: &str =
    "note: model catalogs change rapidly — verify against your provider's documentation before relying on these.";

/// `swarm provider models [TYPE]` — print suggested model ids per provider
/// type (legacy aliases marked), always closing with the staleness nudge.
fn provider_models(args: &[String], out: &mut dyn Write) -> Result<i32, String> {
    let types: Vec<&str> = match args.first().map(String::as_str) {
        Some(token) => {
            if ProviderType::from_str(token) == ProviderType::None {
                return Err(format!(
                    "Error: invalid provider type `{token}`. Valid types: {}.",
                    VALID_PROVIDER_TYPES.join(", ")
                ));
            }
            vec![token]
        }
        None => VALID_PROVIDER_TYPES.to_vec(),
    };
    let mut write =
        |line: String| writeln!(out, "{line}").map_err(|e| format!("Error writing output: {e}"));
    for token in types {
        let provider_type = ProviderType::from_str(token);
        write(format!("== {token} =="))?;
        let suggested = provider_type.suggested_models();
        let legacy = provider_type.legacy_model_aliases();
        if suggested.is_empty() && legacy.is_empty() {
            write("(no suggested models — any model id the endpoint serves works)".to_string())?;
        }
        for model in suggested {
            write(format!("  {model}"))?;
        }
        for model in legacy {
            write(format!("  {model} (legacy)"))?;
        }
    }
    write(MODEL_CATALOG_NUDGE.to_string())?;
    Ok(0)
}

fn provider_add(
    registry: &ProviderRegistry,
    args: &[String],
    out: &mut dyn Write,
) -> Result<i32, String> {
    let mut id: Option<String> = None;
    let mut provider_type: Option<String> = None;
    let mut name: Option<String> = None;
    let mut endpoint: Option<String> = None;
    let mut models: Vec<String> = Vec::new();
    let mut models_given = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--type" => provider_type = Some(required_value(&mut iter, "--type")?),
            "--name" => name = Some(required_value(&mut iter, "--name")?),
            "--endpoint" => endpoint = Some(required_value(&mut iter, "--endpoint")?),
            "--models" => {
                models_given = true;
                models = required_value(&mut iter, "--models")?
                    .split(',')
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .map(ToString::to_string)
                    .collect();
            }
            other if other.starts_with("--") => {
                return Err(format!(
                    "Error: unknown option `{other}` for `provider add`.\n{PROVIDER_USAGE}"
                ));
            }
            other if id.is_none() => id = Some(other.to_string()),
            other => {
                return Err(format!(
                    "Error: unexpected argument `{other}` for `provider add`.\n{PROVIDER_USAGE}"
                ));
            }
        }
    }

    let id =
        id.ok_or_else(|| format!("Error: `provider add` requires an ID.\n{PROVIDER_USAGE}"))?;
    let type_token = provider_type
        .ok_or_else(|| format!("Error: `provider add` requires --type.\n{PROVIDER_USAGE}"))?;
    let parsed = ProviderType::from_str(&type_token);
    if parsed == ProviderType::None {
        return Err(format!(
            "Error: invalid provider type `{type_token}`. Valid types: {}.",
            VALID_PROVIDER_TYPES.join(", ")
        ));
    }

    // No --models: default to the type's suggested catalog (when it has one)
    // so the provider is usable immediately — and say so out loud.
    let mut assumed_models = false;
    if !models_given && models.is_empty() {
        let suggested = parsed.suggested_models();
        if !suggested.is_empty() {
            models = suggested.iter().map(ToString::to_string).collect();
            assumed_models = true;
        }
    }

    let existing = registry
        .get(&id)
        .map_err(|e| format!("Error reading provider registry: {e}"))?;

    let models_joined = models.join(", ");
    let mut config =
        ProviderConfig::new(name.unwrap_or_else(|| id.clone()), parsed, endpoint, None);
    config.id = id.clone();
    config.models = models;
    config.enabled = true;
    if let Some(previous) = existing {
        // Re-adding keeps the stored credential (it is resolved into memory
        // here and re-encrypted on write).
        config.api_key = previous.api_key;
        config.created_at = previous.created_at;
    }
    registry
        .upsert(config)
        .map_err(|e| format!("Error saving provider `{id}`: {e}"))?;
    writeln!(out, "saved provider `{id}` ({})", parsed.as_str())
        .map_err(|e| format!("Error writing output: {e}"))?;
    if assumed_models {
        writeln!(
            out,
            "no --models given; assumed suggested models for {}: {}\n{MODEL_CATALOG_NUDGE}",
            parsed.as_str(),
            models_joined
        )
        .map_err(|e| format!("Error writing output: {e}"))?;
    }
    Ok(0)
}

fn provider_list(registry: &ProviderRegistry, out: &mut dyn Write) -> Result<i32, String> {
    let providers = registry
        .list()
        .map_err(|e| format!("Error reading provider registry: {e}"))?;
    if providers.is_empty() {
        writeln!(
            out,
            "no providers configured. Add one with `swarm provider add ID --type TYPE`."
        )
        .map_err(|e| format!("Error writing output: {e}"))?;
        return Ok(0);
    }

    let mut rows: Vec<[String; 6]> = vec![[
        "ID".to_string(),
        "TYPE".to_string(),
        "ENDPOINT".to_string(),
        "MODELS".to_string(),
        "ENABLED".to_string(),
        "KEY".to_string(),
    ]];
    for p in &providers {
        rows.push([
            p.id.clone(),
            p.provider_type.as_str().to_string(),
            p.endpoint.clone().unwrap_or_else(|| "-".to_string()),
            if p.models.is_empty() {
                "-".to_string()
            } else {
                p.models.join(",")
            },
            p.enabled.to_string(),
            key_status_label(p).to_string(),
        ]);
    }

    let mut widths = [0usize; 6];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }
    for row in &rows {
        let line = row
            .iter()
            .enumerate()
            .map(|(i, cell)| format!("{cell:<width$}", width = widths[i]))
            .collect::<Vec<_>>()
            .join("  ");
        writeln!(out, "{}", line.trim_end()).map_err(|e| format!("Error writing output: {e}"))?;
    }
    Ok(0)
}

fn provider_key_set(
    registry: &ProviderRegistry,
    args: &[String],
    key_input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<i32, String> {
    let mut id: Option<String> = None;
    let mut from_env: Option<String> = None;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--from-env" => from_env = Some(required_value(&mut iter, "--from-env")?),
            other if other.starts_with("--") => {
                return Err(format!(
                    "Error: unknown option `{other}` for `provider key set`.\n{PROVIDER_USAGE}"
                ));
            }
            other if id.is_none() => id = Some(other.to_string()),
            _ => {
                return Err(
                    "Error: `provider key set` takes the key on STDIN, never as an argument \
                     (argv leaks into process listings and shell history). \
                     Pipe it in (`pbpaste | swarm provider key set ID`) or use --from-env VAR."
                        .to_string(),
                );
            }
        }
    }
    let id =
        id.ok_or_else(|| format!("Error: `provider key set` requires an ID.\n{PROVIDER_USAGE}"))?;

    let mut config = registry
        .get(&id)
        .map_err(|e| format!("Error reading provider registry: {e}"))?
        .ok_or_else(|| {
            format!("Error: provider `{id}` not found. Add it first with `swarm provider add {id} --type TYPE`.")
        })?;

    let key = match from_env {
        Some(var) => std::env::var(&var)
            .map_err(|_| format!("Error: environment variable `{var}` is not set."))?,
        None => {
            let mut line = String::new();
            key_input
                .read_line(&mut line)
                .map_err(|e| format!("Error reading key from stdin: {e}"))?;
            line.trim().to_string()
        }
    };
    if key.is_empty() {
        return Err("Error: no key provided (stdin was empty).".to_string());
    }

    let key_len = key.len();
    config.api_key = Some(key);
    registry
        .update(config)
        .map_err(|e| format!("Error storing key for provider `{id}`: {e}"))?;

    let after = registry
        .get(&id)
        .map_err(|e| format!("Error reading provider registry: {e}"))?
        .ok_or_else(|| format!("Error: provider `{id}` vanished during update."))?;
    writeln!(
        out,
        "stored key for `{id}` ({key_len} chars); key status: {}",
        key_status_label(&after)
    )
    .map_err(|e| format!("Error writing output: {e}"))?;
    Ok(0)
}

fn provider_key_check(
    registry: &ProviderRegistry,
    args: &[String],
    out: &mut dyn Write,
) -> Result<i32, String> {
    let id = args
        .first()
        .ok_or_else(|| format!("Error: `provider key check` requires an ID.\n{PROVIDER_USAGE}"))?;
    let config = registry
        .get(id)
        .map_err(|e| format!("Error reading provider registry: {e}"))?
        .ok_or_else(|| format!("Error: provider `{id}` not found."))?;
    let label = key_status_label(&config);
    let next_step = match (config.key_status(), config.has_encrypted_key) {
        (KeyStatus::Healthy, true) => "key is stored in the encrypted vault; nothing to do.",
        (KeyStatus::Healthy, false) => {
            "key comes from the environment; persist it with `swarm provider key set` if you want it in the vault."
        }
        (KeyStatus::Absent, _) => {
            "no key available; set one with `swarm provider key set ID` (key on stdin)."
        }
        (KeyStatus::Stranded, _) => {
            "stored ciphertext cannot be decrypted (master key changed?); re-store with `swarm provider key set ID`."
        }
    };
    writeln!(
        out,
        "provider `{id}` key status: {label}\nnext step: {next_step}"
    )
    .map_err(|e| format!("Error writing output: {e}"))?;
    Ok(0)
}

fn provider_remove(
    registry: &ProviderRegistry,
    args: &[String],
    out: &mut dyn Write,
) -> Result<i32, String> {
    let id = args
        .first()
        .ok_or_else(|| format!("Error: `provider remove` requires an ID.\n{PROVIDER_USAGE}"))?;
    if registry
        .get(id)
        .map_err(|e| format!("Error reading provider registry: {e}"))?
        .is_none()
    {
        return Err(format!("Error: provider `{id}` not found."));
    }
    registry
        .delete(id)
        .map_err(|e| format!("Error removing provider `{id}`: {e}"))?;
    writeln!(out, "removed provider `{id}`").map_err(|e| format!("Error writing output: {e}"))?;
    Ok(0)
}

fn required_value(iter: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, String> {
    iter.next()
        .map(ToString::to_string)
        .ok_or_else(|| format!("Error: {flag} requires a value."))
}

/// Human label for a provider's key status, distinguishing an env-var-resolved
/// key ("env") from one held in the encrypted vault ("healthy").
pub(crate) fn key_status_label(config: &ProviderConfig) -> &'static str {
    match config.key_status() {
        KeyStatus::Healthy if !config.has_encrypted_key => "env",
        KeyStatus::Healthy => "healthy",
        KeyStatus::Absent => "absent",
        KeyStatus::Stranded => "stranded",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use swarm_manager::provider::crypto::{env_key_for, KeychainVault};
    use tempfile::tempdir;

    fn test_vault() -> KeychainVault {
        let mut key = [0u8; 32];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = i as u8;
        }
        KeychainVault::with_key(key)
    }

    fn registry_in(dir: &std::path::Path) -> ProviderRegistry {
        ProviderRegistry::open_with_vault(&dir.to_path_buf(), test_vault()).unwrap()
    }

    fn run(
        registry: &ProviderRegistry,
        args: &[&str],
        stdin: &str,
    ) -> (Result<i32, String>, String) {
        let args: Vec<String> = args.iter().map(ToString::to_string).collect();
        let mut input = Cursor::new(stdin.as_bytes().to_vec());
        let mut out = Vec::new();
        let code = run_provider(registry, &args, &mut input, &mut out);
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn add_then_list_shows_row() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        let (code, _) = run(
            &registry,
            &[
                "add",
                "api",
                "--type",
                "openai",
                "--endpoint",
                "https://example.test/v1",
                "--models",
                "a,b",
            ],
            "",
        );
        assert_eq!(code.unwrap(), 0);

        let (code, output) = run(&registry, &["list"], "");
        assert_eq!(code.unwrap(), 0);
        let row = output.lines().nth(1).unwrap();
        assert!(row.contains("api"), "{output}");
        assert!(row.contains("openai"), "{output}");
        assert!(row.contains("https://example.test/v1"), "{output}");
        assert!(row.contains("a,b"), "{output}");
        assert!(row.contains("true"), "{output}");
        assert!(row.contains("absent"), "{output}");
    }

    #[test]
    fn add_with_invalid_type_lists_valid_types() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        let err = run(&registry, &["add", "api", "--type", "frobnicator"], "")
            .0
            .unwrap_err();
        assert!(err.contains("invalid provider type `frobnicator`"), "{err}");
        for t in VALID_PROVIDER_TYPES {
            assert!(err.contains(t), "missing `{t}` in: {err}");
        }
    }

    #[test]
    fn key_set_reads_stdin_and_never_echoes_the_key() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        run(&registry, &["add", "api", "--type", "openai"], "")
            .0
            .unwrap();

        let (code, output) = run(&registry, &["key", "set", "api"], "sk-super-secret-key\n");
        assert_eq!(code.unwrap(), 0);
        assert!(
            !output.contains("sk-super-secret-key"),
            "key echoed: {output}"
        );
        assert!(
            output.contains("stored key for `api` (19 chars)"),
            "{output}"
        );
        assert!(output.contains("healthy"), "{output}");

        let p = registry.get("api").unwrap().unwrap();
        assert_eq!(p.key_status(), KeyStatus::Healthy);
        assert_eq!(p.api_key.as_deref(), Some("sk-super-secret-key"));

        let (_, list_output) = run(&registry, &["list"], "");
        assert!(!list_output.contains("sk-super-secret-key"));
        assert!(list_output.contains("healthy"), "{list_output}");
    }

    #[test]
    fn key_set_rejects_key_in_argv() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        run(&registry, &["add", "api", "--type", "openai"], "")
            .0
            .unwrap();
        let err = run(&registry, &["key", "set", "api", "sk-oops"], "")
            .0
            .unwrap_err();
        assert!(err.contains("STDIN"), "{err}");
        assert!(err.contains("--from-env"), "{err}");
        // The misplaced key must not have been stored.
        assert_eq!(
            registry.get("api").unwrap().unwrap().key_status(),
            KeyStatus::Absent
        );
    }

    #[test]
    fn key_set_from_env_copies_into_vault() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        run(&registry, &["add", "api2", "--type", "anthropic"], "")
            .0
            .unwrap();
        // Unique var name: no other test touches it, so no env race.
        std::env::set_var("SWARM_TEST_KEY_SOURCE_VAR", "sk-from-source-env");
        let (code, output) = run(
            &registry,
            &[
                "key",
                "set",
                "api2",
                "--from-env",
                "SWARM_TEST_KEY_SOURCE_VAR",
            ],
            "",
        );
        std::env::remove_var("SWARM_TEST_KEY_SOURCE_VAR");
        assert_eq!(code.unwrap(), 0);
        assert!(!output.contains("sk-from-source-env"), "{output}");
        let p = registry.get("api2").unwrap().unwrap();
        assert_eq!(p.api_key.as_deref(), Some("sk-from-source-env"));
        assert_eq!(p.key_status(), KeyStatus::Healthy);
    }

    #[test]
    fn key_set_for_unknown_provider_errors() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        let err = run(&registry, &["key", "set", "ghost"], "sk-x\n")
            .0
            .unwrap_err();
        assert!(err.contains("provider `ghost` not found"), "{err}");
    }

    #[test]
    fn env_fallback_shows_env_in_list_and_check() {
        let dir = tempdir().unwrap();
        // Keychain-free vault: nothing persists; only the env fallback works.
        let registry = ProviderRegistry::open_with_vault(
            &dir.path().to_path_buf(),
            KeychainVault::without_keychain(),
        )
        .unwrap();
        run(&registry, &["add", "envprov", "--type", "openai"], "")
            .0
            .unwrap();

        let var = env_key_for("envprov");
        std::env::set_var(&var, "sk-runtime-only");
        let (_, list_output) = run(&registry, &["list"], "");
        let (_, check_output) = run(&registry, &["key", "check", "envprov"], "");
        std::env::remove_var(&var);

        assert!(!list_output.contains("sk-runtime-only"));
        assert!(
            list_output.lines().nth(1).unwrap().contains("env"),
            "{list_output}"
        );
        assert!(check_output.contains("key status: env"), "{check_output}");
        assert!(check_output.contains("next step:"), "{check_output}");
    }

    #[test]
    fn key_check_reports_absent_with_next_step() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        run(&registry, &["add", "api", "--type", "gemini"], "")
            .0
            .unwrap();
        let (code, output) = run(&registry, &["key", "check", "api"], "");
        assert_eq!(code.unwrap(), 0);
        assert!(output.contains("key status: absent"), "{output}");
        assert!(output.contains("swarm provider key set"), "{output}");
    }

    #[test]
    fn remove_deletes_and_missing_id_errors() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        run(&registry, &["add", "api", "--type", "ollama"], "")
            .0
            .unwrap();
        let (code, output) = run(&registry, &["remove", "api"], "");
        assert_eq!(code.unwrap(), 0);
        assert!(output.contains("removed provider `api`"));
        assert!(registry.list().unwrap().is_empty());

        let err = run(&registry, &["remove", "api"], "").0.unwrap_err();
        assert!(err.contains("provider `api` not found"), "{err}");
    }

    #[test]
    fn re_add_preserves_stored_key() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        run(&registry, &["add", "api", "--type", "openai"], "")
            .0
            .unwrap();
        run(&registry, &["key", "set", "api"], "sk-keep-me\n")
            .0
            .unwrap();

        // Re-add with new metadata; the credential must survive.
        run(
            &registry,
            &["add", "api", "--type", "openai", "--name", "Renamed"],
            "",
        )
        .0
        .unwrap();
        let p = registry.get("api").unwrap().unwrap();
        assert_eq!(p.name, "Renamed");
        assert_eq!(p.api_key.as_deref(), Some("sk-keep-me"));
        assert_eq!(p.key_status(), KeyStatus::Healthy);
    }

    #[test]
    fn split_data_dir_extracts_override() {
        let raw: Vec<String> = ["list", "--data-dir", "/tmp/x"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let (rest, dir) = split_data_dir(&raw).unwrap();
        assert_eq!(rest, vec!["list".to_string()]);
        assert_eq!(dir, Some(PathBuf::from("/tmp/x")));
        assert!(split_data_dir(&["--data-dir".to_string()]).is_err());
    }

    #[test]
    fn models_lists_all_types_with_legacy_markers_and_nudge() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        let (code, output) = run(&registry, &["models"], "");
        assert_eq!(code.unwrap(), 0);
        for t in VALID_PROVIDER_TYPES {
            assert!(output.contains(&format!("== {t} ==")), "{output}");
        }
        assert!(output.contains("gpt-5.5"), "{output}");
        assert!(output.contains("gpt-4o (legacy)"), "{output}");
        assert!(output.contains("deepseek-chat (legacy)"), "{output}");
        // Types with no catalog say so instead of printing nothing.
        assert!(output.contains("no suggested models"), "{output}");
        assert!(output.trim_end().ends_with(MODEL_CATALOG_NUDGE), "{output}");
    }

    #[test]
    fn models_with_single_type_scopes_output() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        let (code, output) = run(&registry, &["models", "deepseek"], "");
        assert_eq!(code.unwrap(), 0);
        assert!(output.contains("== deepseek =="), "{output}");
        assert!(output.contains("deepseek-v4-flash"), "{output}");
        assert!(output.contains("deepseek-reasoner (legacy)"), "{output}");
        assert!(!output.contains("== openai =="), "{output}");
        assert!(output.contains(MODEL_CATALOG_NUDGE), "{output}");
    }

    #[test]
    fn models_with_unknown_type_errors_listing_valid_types() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        let err = run(&registry, &["models", "frobnicator"], "")
            .0
            .unwrap_err();
        assert!(err.contains("invalid provider type `frobnicator`"), "{err}");
        for t in VALID_PROVIDER_TYPES {
            assert!(err.contains(t), "missing `{t}` in: {err}");
        }
    }

    #[test]
    fn add_without_models_assumes_suggested_and_prints_nudge() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        let (code, output) = run(&registry, &["add", "ds", "--type", "deepseek"], "");
        assert_eq!(code.unwrap(), 0);
        assert!(
            output.contains(
                "assumed suggested models for deepseek: deepseek-v4-flash, deepseek-v4-pro"
            ),
            "{output}"
        );
        assert!(output.contains(MODEL_CATALOG_NUDGE), "{output}");
        let stored = registry.get("ds").unwrap().unwrap();
        assert_eq!(
            stored.models,
            vec![
                "deepseek-v4-flash".to_string(),
                "deepseek-v4-pro".to_string()
            ]
        );
    }

    #[test]
    fn add_without_models_for_catalogless_type_stays_empty_and_quiet() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        let (code, output) = run(&registry, &["add", "local", "--type", "ollama"], "");
        assert_eq!(code.unwrap(), 0);
        assert!(!output.contains("assumed"), "{output}");
        assert!(registry.get("local").unwrap().unwrap().models.is_empty());
    }

    #[test]
    fn add_with_explicit_models_does_not_assume() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        let (code, output) = run(
            &registry,
            &["add", "ds", "--type", "deepseek", "--models", "my-model"],
            "",
        );
        assert_eq!(code.unwrap(), 0);
        assert!(!output.contains("assumed"), "{output}");
        assert_eq!(
            registry.get("ds").unwrap().unwrap().models,
            vec!["my-model".to_string()]
        );
    }

    #[test]
    fn unknown_subcommand_errors_with_usage() {
        let dir = tempdir().unwrap();
        let registry = registry_in(dir.path());
        let err = run(&registry, &["frob"], "").0.unwrap_err();
        assert!(err.contains("usage: swarm provider"), "{err}");
    }
}
