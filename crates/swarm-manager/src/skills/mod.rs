//! Agent-Skills loader: `SKILL.md` files that inject system-prompt guidance
//! and (optionally) gate the agent's tool set.
//!
//! A skill is a single `SKILL.md` markdown file with a leading YAML-ish
//! frontmatter block. Only three keys are honoured — `name`, `description`,
//! and `allowed-tools` — and the markdown body after the closing fence is the
//! prompt fragment injected into the system prompt.
//!
//! ## Canonical on-disk layout
//!
//! Skills use the subdir-per-skill convention to match the Agent-Skills
//! ecosystem:
//!
//! ```text
//! <dir>/
//!   <skill-name>/
//!     SKILL.md
//! ```
//!
//! [`load_skills`] scans a list of directories. Later directories override
//! earlier ones by skill name, so a caller can pass `[home_dir, project_dir]`
//! and let a project-local skill shadow a same-named home skill.
//!
//! ## Selection and composition
//!
//! [`SkillSet`] takes the loaded skills plus a list of requested names and
//! resolves the selected subset. It exposes two outputs:
//!
//! - [`SkillSet::compose_system_prompt`] appends each selected skill's body to
//!   a base prompt under a `## Skill: <name>` header, in stable order.
//! - [`SkillSet::allowed_tools`] returns the UNION of every selected skill's
//!   `allowed-tools`, or `None` when no selected skill restricts (meaning "no
//!   gating — all tools").
//!
//! Malformed skills never panic: [`load_skills`] folds parse failures into a
//! [`SkillLoadIssue`] list the caller can surface, and unknown requested
//! names become a recoverable [`SkillSelectionIssue`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A loaded skill: its frontmatter metadata plus the markdown body that is
/// injected into the system prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// The skill name (from the `name` frontmatter key).
    pub name: String,
    /// A human-readable one-line description.
    pub description: String,
    /// The tools this skill permits. `None` means the skill does not restrict
    /// the tool set; `Some(set)` gates to exactly those tool names.
    pub allowed_tools: Option<Vec<String>>,
    /// The markdown body after the closing frontmatter fence — the prompt
    /// fragment.
    pub body: String,
    /// The `SKILL.md` path this skill was parsed from.
    pub source: PathBuf,
}

/// A typed frontmatter / structure parse failure. Always loud — never a silent
/// fallback.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkillError {
    /// The file did not begin with a `---` frontmatter fence.
    #[error("skill is missing the opening `---` frontmatter fence")]
    MissingOpeningFence,
    /// The opening fence was never closed by a second `---` line.
    #[error("skill is missing the closing `---` frontmatter fence")]
    MissingClosingFence,
    /// The required `name` key was absent (or empty) in the frontmatter.
    #[error("skill frontmatter is missing the required `name` key")]
    MissingName,
}

/// A skill that could not be loaded, paired with the reason. Collected rather
/// than thrown so one bad skill never blocks the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillLoadIssue {
    /// The `SKILL.md` path that failed to load.
    pub source: PathBuf,
    /// Why it failed.
    pub error: SkillError,
}

/// A requested skill name that did not resolve to any loaded skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSelectionIssue {
    /// The requested name that was not found among the loaded skills.
    pub requested: String,
}

/// Parse a `SKILL.md`'s text into a [`Skill`].
///
/// The format: the file MUST begin with a line that is exactly `---`, followed
/// by `key: value` lines, terminated by another line that is exactly `---`.
/// Everything after the closing fence is the body. Only `name`, `description`,
/// and `allowed-tools` are honoured; unknown keys are ignored.
///
/// `allowed-tools` accepts a comma-separated list (`a, b, c`) OR a simple
/// inline list (`[a, b, c]`); both forms are trimmed.
pub fn parse_skill(text: &str, source: impl Into<PathBuf>) -> Result<Skill, SkillError> {
    let source = source.into();

    // The opening fence must be the very first line (allowing a leading BOM or
    // surrounding whitespace on that line only).
    let mut lines = text.lines();
    let first = lines.next().ok_or(SkillError::MissingOpeningFence)?;
    if first.trim() != "---" {
        return Err(SkillError::MissingOpeningFence);
    }

    let mut name: Option<String> = None;
    let mut description = String::new();
    let mut allowed_tools: Option<Vec<String>> = None;
    let mut closed = false;

    // Track how many bytes of `text` the frontmatter (incl. both fences plus
    // their trailing newlines) consumes, so the remainder is the verbatim body.
    let mut consumed = first.len();
    consumed += newline_len(text, consumed);

    for line in lines {
        let line_start = consumed;
        consumed += line.len();
        consumed += newline_len(text, consumed);

        if line.trim() == "---" {
            closed = true;
            break;
        }

        let _ = line_start;
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "name" => {
                    if !value.is_empty() {
                        name = Some(value.to_string());
                    }
                }
                "description" => description = value.to_string(),
                "allowed-tools" => allowed_tools = Some(parse_allowed_tools(value)),
                _ => { /* unknown key — ignored */ }
            }
        }
    }

    if !closed {
        return Err(SkillError::MissingClosingFence);
    }
    let name = name.ok_or(SkillError::MissingName)?;

    let body = text.get(consumed..).unwrap_or("").to_string();

    Ok(Skill {
        name,
        description,
        allowed_tools,
        body,
        source,
    })
}

/// Length (in bytes) of the newline sequence at `text[offset..]`, or 0 if there
/// is none (end of input). Handles both `\n` and `\r\n`.
fn newline_len(text: &str, offset: usize) -> usize {
    let rest = &text.as_bytes()[offset.min(text.len())..];
    match rest.first() {
        Some(b'\r') if rest.get(1) == Some(&b'\n') => 2,
        Some(b'\n') => 1,
        _ => 0,
    }
}

/// Parse the `allowed-tools` value. Accepts `a, b, c` or `[a, b, c]`; trims and
/// drops empties. An empty value yields an empty vector (an explicit
/// "restrict to nothing").
fn parse_allowed_tools(value: &str) -> Vec<String> {
    let inner = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(value);
    inner
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Scan each directory for `<dir>/<name>/SKILL.md` and load the skills.
///
/// Later directories override earlier ones by skill name (so a `[home, project]`
/// ordering lets the project win). Malformed `SKILL.md` files become
/// [`SkillLoadIssue`]s rather than aborting the scan. Directories that do not
/// exist are skipped silently.
pub fn load_skills(dirs: &[PathBuf]) -> (Vec<Skill>, Vec<SkillLoadIssue>) {
    // Keyed by skill name; later dirs overwrite earlier entries.
    let mut by_name: std::collections::BTreeMap<String, Skill> = std::collections::BTreeMap::new();
    let mut issues = Vec::new();

    for dir in dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => continue, // missing/unreadable dir — skip
        };

        // Sort sub-entries for deterministic issue ordering within a dir.
        let mut subdirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        subdirs.sort();

        for subdir in subdirs {
            let skill_file = subdir.join("SKILL.md");
            if !skill_file.is_file() {
                continue;
            }
            let text = match std::fs::read_to_string(&skill_file) {
                Ok(text) => text,
                Err(_) => continue,
            };
            match parse_skill(&text, &skill_file) {
                Ok(skill) => {
                    by_name.insert(skill.name.clone(), skill);
                }
                Err(error) => issues.push(SkillLoadIssue {
                    source: skill_file,
                    error,
                }),
            }
        }
    }

    (by_name.into_values().collect(), issues)
}

/// A resolved selection of skills, ready to compose a system prompt and report
/// the gated tool set.
#[derive(Debug, Clone, Default)]
pub struct SkillSet {
    /// The selected skills, in stable (request) order.
    selected: Vec<Skill>,
    /// Requested names that did not resolve to a loaded skill.
    unknown: Vec<SkillSelectionIssue>,
}

impl SkillSet {
    /// Resolve `requested` names against the `loaded` skills.
    ///
    /// Selected skills are returned in the order requested (de-duplicated,
    /// first occurrence wins). Unknown names are collected as recoverable
    /// issues, never silently dropped.
    pub fn resolve(loaded: &[Skill], requested: &[String]) -> Self {
        let mut selected = Vec::new();
        let mut unknown = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();

        for name in requested {
            if !seen.insert(name.as_str()) {
                continue;
            }
            match loaded.iter().find(|s| &s.name == name) {
                Some(skill) => selected.push(skill.clone()),
                None => unknown.push(SkillSelectionIssue {
                    requested: name.clone(),
                }),
            }
        }

        Self { selected, unknown }
    }

    /// The selected skills, in request order.
    pub fn selected(&self) -> &[Skill] {
        &self.selected
    }

    /// Requested names that resolved to nothing.
    pub fn unknown(&self) -> &[SkillSelectionIssue] {
        &self.unknown
    }

    /// Whether any skill was selected.
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    /// Compose the system prompt: the `base`, then each selected skill's body
    /// under a `## Skill: <name>` header, in stable order.
    ///
    /// When no skill is selected the base prompt is returned unchanged.
    pub fn compose_system_prompt(&self, base: &str) -> String {
        if self.selected.is_empty() {
            return base.to_string();
        }
        let mut out = base.to_string();
        for skill in &self.selected {
            out.push_str("\n\n## Skill: ");
            out.push_str(&skill.name);
            out.push('\n');
            let body = skill.body.trim();
            if !body.is_empty() {
                out.push('\n');
                out.push_str(body);
            }
        }
        out
    }

    /// The UNION of every selected skill's `allowed-tools`.
    ///
    /// Returns `None` when NO selected skill restricts the tool set (meaning
    /// "no gating — all tools stay available"). When at least one skill
    /// restricts, the union of all restricting skills' tool names is returned —
    /// a skill that does not restrict contributes nothing and does not widen
    /// the gate back to "all".
    pub fn allowed_tools(&self) -> Option<HashSet<String>> {
        let mut union: Option<HashSet<String>> = None;
        for skill in &self.selected {
            if let Some(tools) = &skill.allowed_tools {
                let set = union.get_or_insert_with(HashSet::new);
                for tool in tools {
                    set.insert(tool.clone());
                }
            }
        }
        union
    }
}

/// Resolve a `<dir>/<name>/SKILL.md` path for a skill name. Helper for callers
/// (e.g. the CLI) that want to report or locate a skill's canonical file.
pub fn skill_file_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(name).join("SKILL.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_md(name: &str, desc: &str, allowed: Option<&str>, body: &str) -> String {
        let mut s = String::from("---\n");
        s.push_str(&format!("name: {name}\n"));
        s.push_str(&format!("description: {desc}\n"));
        if let Some(a) = allowed {
            s.push_str(&format!("allowed-tools: {a}\n"));
        }
        s.push_str("---\n");
        s.push_str(body);
        s
    }

    fn write_skill(dir: &Path, name: &str, contents: &str) {
        let sub = dir.join(name);
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("SKILL.md"), contents).unwrap();
    }

    // ── Frontmatter parsing ─────────────────────────────────────────────

    #[test]
    fn parse_full_skill() {
        let text = skill_md(
            "reviewer",
            "Careful code review",
            Some("read_file, exec"),
            "Be thorough.\nCheck edge cases.\n",
        );
        let skill = parse_skill(&text, "/skills/reviewer/SKILL.md").unwrap();
        assert_eq!(skill.name, "reviewer");
        assert_eq!(skill.description, "Careful code review");
        assert_eq!(
            skill.allowed_tools,
            Some(vec!["read_file".to_string(), "exec".to_string()])
        );
        assert_eq!(skill.body, "Be thorough.\nCheck edge cases.\n");
        assert_eq!(skill.source, PathBuf::from("/skills/reviewer/SKILL.md"));
    }

    #[test]
    fn parse_skill_without_allowed_tools_is_unrestricted() {
        let text = skill_md("doc", "Writes docs", None, "Write clearly.");
        let skill = parse_skill(&text, "x").unwrap();
        assert_eq!(skill.allowed_tools, None);
        assert_eq!(skill.body, "Write clearly.");
    }

    #[test]
    fn parse_skill_inline_list_form_for_allowed_tools() {
        let text = skill_md("r", "d", Some("[read_file, list_dir]"), "body");
        let skill = parse_skill(&text, "x").unwrap();
        assert_eq!(
            skill.allowed_tools,
            Some(vec!["read_file".to_string(), "list_dir".to_string()])
        );
    }

    #[test]
    fn parse_skill_ignores_unknown_keys() {
        let text = "---\nname: k\ndescription: d\nmystery: 42\nrank: high\n---\nbody";
        let skill = parse_skill(text, "x").unwrap();
        assert_eq!(skill.name, "k");
        assert_eq!(skill.description, "d");
    }

    #[test]
    fn parse_skill_missing_name_is_error() {
        let text = "---\ndescription: no name here\n---\nbody";
        assert_eq!(parse_skill(text, "x"), Err(SkillError::MissingName));
    }

    #[test]
    fn parse_skill_empty_name_value_is_missing_name() {
        let text = "---\nname:   \ndescription: d\n---\nbody";
        assert_eq!(parse_skill(text, "x"), Err(SkillError::MissingName));
    }

    #[test]
    fn parse_skill_missing_closing_fence_is_error() {
        let text = "---\nname: k\ndescription: d\nbody with no closing fence\n";
        assert_eq!(parse_skill(text, "x"), Err(SkillError::MissingClosingFence));
    }

    #[test]
    fn parse_skill_missing_opening_fence_is_error() {
        let text = "name: k\ndescription: d\n---\nbody";
        assert_eq!(parse_skill(text, "x"), Err(SkillError::MissingOpeningFence));
    }

    #[test]
    fn parse_skill_handles_crlf_line_endings() {
        let text = "---\r\nname: k\r\ndescription: d\r\n---\r\nthe body\r\n";
        let skill = parse_skill(text, "x").unwrap();
        assert_eq!(skill.name, "k");
        assert_eq!(skill.body, "the body\r\n");
    }

    #[test]
    fn parse_skill_empty_allowed_tools_restricts_to_nothing() {
        let text = skill_md("r", "d", Some("  "), "body");
        let skill = parse_skill(&text, "x").unwrap();
        assert_eq!(skill.allowed_tools, Some(Vec::<String>::new()));
    }

    // ── Loader ──────────────────────────────────────────────────────────

    #[test]
    fn load_skills_reads_subdir_layout() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "alpha",
            &skill_md("alpha", "first", Some("read_file"), "Alpha body"),
        );
        write_skill(
            dir.path(),
            "beta",
            &skill_md("beta", "second", None, "Beta body"),
        );

        let (skills, issues) = load_skills(&[dir.path().to_path_buf()]);
        assert!(issues.is_empty());
        assert_eq!(skills.len(), 2);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }

    #[test]
    fn load_skills_project_overrides_home() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        write_skill(
            home.path(),
            "shared",
            &skill_md("shared", "home version", None, "HOME BODY"),
        );
        write_skill(
            project.path(),
            "shared",
            &skill_md("shared", "project version", None, "PROJECT BODY"),
        );

        // [home, project] → project wins.
        let (skills, issues) =
            load_skills(&[home.path().to_path_buf(), project.path().to_path_buf()]);
        assert!(issues.is_empty());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "project version");
        assert_eq!(skills[0].body, "PROJECT BODY");
    }

    #[test]
    fn load_skills_malformed_becomes_issue_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "good", &skill_md("good", "d", None, "ok"));
        // Missing closing fence.
        write_skill(dir.path(), "bad", "---\nname: bad\nno closing fence\n");

        let (skills, issues) = load_skills(&[dir.path().to_path_buf()]);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "good");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].error, SkillError::MissingClosingFence);
        assert!(issues[0].source.ends_with("bad/SKILL.md"));
    }

    #[test]
    fn load_skills_skips_missing_dir() {
        let (skills, issues) = load_skills(&[PathBuf::from("/no/such/skills/dir")]);
        assert!(skills.is_empty());
        assert!(issues.is_empty());
    }

    // ── SkillSet selection + composition ────────────────────────────────

    fn skill(name: &str, allowed: Option<Vec<&str>>, body: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: format!("desc {name}"),
            allowed_tools: allowed.map(|v| v.into_iter().map(str::to_string).collect()),
            body: body.to_string(),
            source: PathBuf::from(format!("/skills/{name}/SKILL.md")),
        }
    }

    #[test]
    fn skillset_compose_orders_and_headers() {
        let loaded = vec![
            skill("alpha", None, "Alpha guidance."),
            skill("beta", None, "Beta guidance."),
        ];
        // Request beta before alpha → output follows request order.
        let set = SkillSet::resolve(&loaded, &["beta".into(), "alpha".into()]);
        let prompt = set.compose_system_prompt("BASE PROMPT");
        assert_eq!(
            prompt,
            "BASE PROMPT\n\n## Skill: beta\n\nBeta guidance.\n\n## Skill: alpha\n\nAlpha guidance."
        );
    }

    #[test]
    fn skillset_compose_no_selection_returns_base() {
        let set = SkillSet::resolve(&[], &[]);
        assert_eq!(set.compose_system_prompt("BASE"), "BASE");
        assert!(set.is_empty());
    }

    #[test]
    fn skillset_resolve_deduplicates_requests() {
        let loaded = vec![skill("alpha", None, "A")];
        let set = SkillSet::resolve(&loaded, &["alpha".into(), "alpha".into()]);
        assert_eq!(set.selected().len(), 1);
    }

    #[test]
    fn skillset_unknown_name_is_recoverable_issue() {
        let loaded = vec![skill("alpha", None, "A")];
        let set = SkillSet::resolve(&loaded, &["alpha".into(), "ghost".into()]);
        assert_eq!(set.selected().len(), 1);
        assert_eq!(set.unknown().len(), 1);
        assert_eq!(set.unknown()[0].requested, "ghost");
    }

    // ── allowed_tools union ─────────────────────────────────────────────

    #[test]
    fn allowed_tools_none_when_no_skill_restricts() {
        let loaded = vec![skill("a", None, ""), skill("b", None, "")];
        let set = SkillSet::resolve(&loaded, &["a".into(), "b".into()]);
        assert_eq!(set.allowed_tools(), None);
    }

    #[test]
    fn allowed_tools_union_of_overlapping_and_disjoint() {
        let loaded = vec![
            skill("a", Some(vec!["read_file", "exec"]), ""),
            skill("b", Some(vec!["exec", "list_dir"]), ""),
        ];
        let set = SkillSet::resolve(&loaded, &["a".into(), "b".into()]);
        let tools = set.allowed_tools().unwrap();
        let expected: HashSet<String> = ["read_file", "exec", "list_dir"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(tools, expected);
    }

    #[test]
    fn allowed_tools_restricting_skill_gates_even_with_unrestricted_sibling() {
        // One skill restricts to read_file, the other does not restrict at all.
        // The restricting skill must still gate — an unrestricted sibling does
        // NOT widen the gate back to "all tools".
        let loaded = vec![
            skill("locked", Some(vec!["read_file"]), ""),
            skill("open", None, ""),
        ];
        let set = SkillSet::resolve(&loaded, &["locked".into(), "open".into()]);
        let tools = set.allowed_tools().unwrap();
        assert_eq!(
            tools,
            ["read_file".to_string()].into_iter().collect::<HashSet<_>>()
        );
    }

    #[test]
    fn skill_file_path_uses_subdir_convention() {
        assert_eq!(
            skill_file_path(Path::new("/root"), "reviewer"),
            PathBuf::from("/root/reviewer/SKILL.md")
        );
    }
}
