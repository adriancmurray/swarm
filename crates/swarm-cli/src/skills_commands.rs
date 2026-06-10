//! `swarm skills list` — enumerate the `SKILL.md` skills the native agent can
//! load.
//!
//! Scans the same directories a native run consults: the home skills dir
//! (`<swarm home>/skills`) layered under the project-local
//! `<cwd>/.swarm/skills`, which overrides it by name. Each loaded skill prints
//! its name, description, source path, and allowed-tools (or "all tools" when
//! it does not restrict). Malformed skills are reported as issues rather than
//! silently skipped.
//!
//! `--dir PATH` overrides the scanned directories (repeatable) — used by tests
//! and by callers who want to inspect an explicit location.

use std::io::{self, Write};
use std::path::PathBuf;

use swarm_manager::{load_skills, Skill, SkillLoadIssue};

const SKILLS_USAGE: &str = "usage: swarm skills list [--dir PATH]...";

/// Entry point for `swarm skills ...`.
pub(crate) fn cmd_skills(raw: &[String]) -> Result<i32, String> {
    let mut out = io::stdout();
    run_skills(raw, &mut out)
}

/// Resolve the default scan directories: home skills dir first (lowest
/// priority), then the project-local `<cwd>/.swarm/skills` (overrides by name).
fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = swarm_store::store::skills_dir() {
        dirs.push(home);
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join(".swarm").join("skills"));
    }
    dirs
}

fn run_skills(raw: &[String], out: &mut dyn Write) -> Result<i32, String> {
    let subcommand = raw.first().map(String::as_str);
    match subcommand {
        Some("list") => {
            let dirs = parse_dirs(&raw[1..])?;
            let dirs = if dirs.is_empty() {
                default_dirs()
            } else {
                dirs
            };
            let (skills, issues) = load_skills(&dirs);
            print_skills(out, &skills, &issues)?;
            Ok(0)
        }
        _ => Err(format!("Error: unknown skills subcommand.\n{SKILLS_USAGE}")),
    }
}

/// Pull repeated `--dir PATH` pairs out of the args.
fn parse_dirs(raw: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut dirs = Vec::new();
    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        if arg == "--dir" {
            let value = iter
                .next()
                .ok_or_else(|| "Error: --dir requires a path.".to_string())?;
            dirs.push(PathBuf::from(value));
        } else {
            return Err(format!("Error: unexpected argument `{arg}`.\n{SKILLS_USAGE}"));
        }
    }
    Ok(dirs)
}

fn print_skills(
    out: &mut dyn Write,
    skills: &[Skill],
    issues: &[SkillLoadIssue],
) -> Result<(), String> {
    let mut w = |line: String| -> Result<(), String> {
        writeln!(out, "{line}").map_err(|e| format!("Error writing output: {e}"))
    };

    if skills.is_empty() {
        w("(no skills found)".to_string())?;
    }
    for skill in skills {
        w(skill.name.clone())?;
        w(format!("  description: {}", skill.description))?;
        w(format!("  source: {}", skill.source.display()))?;
        let tools = match &skill.allowed_tools {
            Some(list) if !list.is_empty() => list.join(", "),
            Some(_) => "(none)".to_string(),
            None => "all tools".to_string(),
        };
        w(format!("  allowed-tools: {tools}"))?;
    }

    if !issues.is_empty() {
        w("\nmalformed skills:".to_string())?;
        for issue in issues {
            w(format!("  ! {}: {}", issue.source.display(), issue.error))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn write_skill(dir: &Path, name: &str, contents: &str) {
        let sub = dir.join(name);
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("SKILL.md"), contents).unwrap();
    }

    fn run(raw: &[&str]) -> (i32, String) {
        let mut buf: Vec<u8> = Vec::new();
        let args: Vec<String> = raw.iter().map(|s| s.to_string()).collect();
        let code = run_skills(&args, &mut buf).unwrap();
        (code, String::from_utf8(buf).unwrap())
    }

    #[test]
    fn list_prints_skill_fields_byte_exact() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "reviewer",
            "---\nname: reviewer\ndescription: Careful review\nallowed-tools: read_file, exec\n---\nbody",
        );
        let src = dir.path().join("reviewer").join("SKILL.md");
        let (code, output) = run(&["list", "--dir", dir.path().to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(
            output,
            format!(
                "reviewer\n  description: Careful review\n  source: {}\n  allowed-tools: read_file, exec\n",
                src.display()
            )
        );
    }

    #[test]
    fn list_reports_all_tools_when_unrestricted() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "doc",
            "---\nname: doc\ndescription: Docs\n---\nbody",
        );
        let (_, output) = run(&["list", "--dir", dir.path().to_str().unwrap()]);
        assert!(output.contains("  allowed-tools: all tools\n"), "{output}");
    }

    #[test]
    fn list_reports_malformed_skills() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "good", "---\nname: good\ndescription: d\n---\nok");
        write_skill(dir.path(), "bad", "---\nname: bad\nno closing fence\n");
        let (_, output) = run(&["list", "--dir", dir.path().to_str().unwrap()]);
        assert!(output.contains("good\n"), "{output}");
        assert!(output.contains("malformed skills:"), "{output}");
        assert!(
            output.contains("closing `---` frontmatter fence"),
            "{output}"
        );
    }

    #[test]
    fn list_empty_dir_says_none_found() {
        let dir = tempfile::tempdir().unwrap();
        let (code, output) = run(&["list", "--dir", dir.path().to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(output, "(no skills found)\n");
    }

    #[test]
    fn unknown_subcommand_is_error() {
        let mut buf: Vec<u8> = Vec::new();
        let err = run_skills(&["wat".to_string()], &mut buf).unwrap_err();
        assert!(err.contains("unknown skills subcommand"), "{err}");
    }
}
