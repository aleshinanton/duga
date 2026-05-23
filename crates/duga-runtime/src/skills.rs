//! SKILL.md loading for duga frontends.
//!
//! Skills are Markdown files with optional YAML frontmatter that provide
//! reusable instructions for coding agents. Skills are loaded from the
//! workspace root `skills/` directory and optional channel-level `skills/`.
//!
//! Architecture (EPIC-29): metadata-only discovery via `SkillIndex` +
//! lazy body loading via `load_skill_body()`. This saves ~14x context tokens
//! compared to eager-loading bodies.
//!
//! - `discover_skills()` — scans directories, parses ONLY frontmatter (fast, ~150 chars/skill).
//! - `load_skill_body()` — loads full body on demand (called when LLM reads SKILL.md).
//! - `load_skills()` — legacy: eager-loads everything. Kept for backward compat.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ── Public types ────────────────────────────────────────────────────────────

/// Lightweight metadata index for a skill (TASK-29.2).
///
/// Parsed from YAML frontmatter only — body is NOT loaded.
/// ~150 chars per skill vs ~2-10KB for the full body.
#[derive(Clone, Debug)]
pub struct SkillIndex {
    pub name: String,
    pub description: Option<String>,
    pub source: SkillSource,
    /// Absolute path to the skill directory (e.g. `/workspace/skills/code-review`).
    pub directory: PathBuf,
    /// Environment prerequisites (TASK-29.4).
    pub requires: Option<SkillRequires>,
    /// If true, the LLM should not invoke this skill autonomously.
    pub disable_model_invocation: bool,
}

/// A fully loaded skill — index + resolved body.
#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: Option<String>,
    pub body: String,
    pub source: SkillSource,
    pub directory: PathBuf,
}

/// Where the skill was loaded from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillSource {
    Workspace,
    Channel,
}

/// Environment prerequisites for a skill (TASK-29.4).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SkillRequires {
    /// Binary names that must be on PATH.
    pub bins: Option<Vec<String>>,
    /// Environment variable templates (e.g. `${GITHUB_TOKEN}`) that must resolve.
    pub env: Option<HashMap<String, String>>,
}

/// Result of environment gating for a skill (TASK-29.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillGate {
    pub available: bool,
    pub missing_bins: Vec<String>,
    pub missing_env: Vec<String>,
}

impl SkillGate {
    /// Always-available gate (no requirements).
    fn always() -> Self {
        Self {
            available: true,
            missing_bins: Vec::new(),
            missing_env: Vec::new(),
        }
    }
}

// ── Internal types ──────────────────────────────────────────────────────────

/// Internal YAML frontmatter for SKILL.md files.
#[derive(Default, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    requires: Option<SkillRequires>,
    #[serde(rename = "disable-model-invocation")]
    disable_model_invocation: Option<bool>,
}

// ── Public API (EPIC-29) ───────────────────────────────────────────────────

/// Scan directories, return metadata-only index. Fast — no body I/O.
///
/// Channel skills override workspace skills on name collision.
pub fn discover_skills(
    global_base: &Path,
    channel_base: Option<&Path>,
) -> Result<Vec<SkillIndex>> {
    let mut map: HashMap<String, SkillIndex> = HashMap::new();

    // Workspace skills first.
    let ws_indexes = discover_skills_from_dir(
        global_base.join("skills"),
        SkillSource::Workspace,
    )?;
    for idx in ws_indexes {
        map.insert(idx.name.clone(), idx);
    }

    // Channel skills override on collision.
    if let Some(channel) = channel_base {
        let ch_indexes = discover_skills_from_dir(
            channel.join("skills"),
            SkillSource::Channel,
        )?;
        for idx in ch_indexes {
            map.insert(idx.name.clone(), idx);
        }
    }

    Ok(map.into_values().collect())
}

/// Load full body for a specific skill. Called when LLM reads the SKILL.md.
///
/// Resolves `{baseDir}` placeholders to the skill's absolute directory path.
pub fn load_skill_body(index: &SkillIndex) -> Result<Skill> {
    let skill_md = index.directory.join("SKILL.md");
    let raw = std::fs::read_to_string(&skill_md)
        .with_context(|| format!("reading {}", skill_md.display()))?;

    let (_frontmatter, body) = extract_frontmatter(&raw);

    // Resolve {baseDir} placeholders.
    let body = body.replace("{baseDir}", &index.directory.to_string_lossy());

    Ok(Skill {
        name: index.name.clone(),
        description: index.description.clone(),
        body,
        source: index.source.clone(),
        directory: index.directory.clone(),
    })
}

/// Check environment prerequisites for a skill (TASK-29.4).
///
/// Verifies that `requires.bins` exist on PATH and `requires.env` template
/// variables resolve to non-empty values. Called at discovery time so the
/// system prompt reflects current environment state.
pub fn gate_skill(index: &SkillIndex) -> SkillGate {
    let Some(ref requires) = index.requires else {
        return SkillGate::always();
    };

    let mut missing_bins = Vec::new();
    let mut missing_env = Vec::new();

    if let Some(ref bins) = requires.bins {
        for bin in bins {
            if which::which(bin).is_err() {
                missing_bins.push(bin.clone());
            }
        }
    }

    if let Some(ref env_vars) = requires.env {
        for (key, template) in env_vars {
            // Extract the env var name from the template "${VAR_NAME}" or "${VAR_NAME:-default}"
            let var_name = extract_env_var_name(template);
            match std::env::var(&var_name) {
                Ok(val) if !val.is_empty() => { /* OK */ }
                _ => missing_env.push(key.clone()),
            }
        }
    }

    SkillGate {
        available: missing_bins.is_empty() && missing_env.is_empty(),
        missing_bins,
        missing_env,
    }
}

/// Format index for system prompt injection (compact markdown list).
///
/// Output ~150 chars/skill instead of ~2KB/skill with bodies.
/// Skips skills with `disable_model_invocation: true`.
/// Unavailable skills are marked with ⚠️.
pub fn format_skills_index_for_prompt(indexes: &[SkillIndex]) -> String {
    if indexes.is_empty() {
        return String::new();
    }

    let mut lines: Vec<String> = vec!["## Available Skills".to_string()];

    for idx in indexes {
        if idx.disable_model_invocation {
            continue;
        }

        let gate = gate_skill(idx);

        let desc_part = idx
            .description
            .as_deref()
            .map(|d| format!(": {}", d))
            .unwrap_or_default();

        let warning = if !gate.available {
            let mut parts: Vec<String> = Vec::new();
            if !gate.missing_bins.is_empty() {
                parts.push(format!("missing {}", gate.missing_bins.join(", ")));
            }
            if !gate.missing_env.is_empty() {
                parts.push(format!("missing env: {}", gate.missing_env.join(", ")));
            }
            format!(" ⚠️ unavailable: {}", parts.join("; "))
        } else {
            String::new()
        };

        lines.push(format!("- **{}**{}{}", idx.name, desc_part, warning));
    }

    lines.push(String::new());
    lines.push(
        "When a task matches a skill's description, use `read` to load its \
         full instructions from the skill's SKILL.md file."
            .to_string(),
    );

    lines.join("\n")
}

/// Legacy: eager-load everything (body included). Kept for backward compat.
///
/// Prefer `discover_skills()` + `format_skills_index_for_prompt()` for new code.
pub fn load_skills(
    workspace: &Path,
    channel_dir: Option<&Path>,
) -> Result<Vec<Skill>> {
    let indexes = discover_skills(workspace, channel_dir)?;
    let mut skills = Vec::with_capacity(indexes.len());
    for idx in &indexes {
        skills.push(load_skill_body(idx)?);
    }
    Ok(skills)
}

/// Format loaded skills as a string for injection into the system prompt.
///
/// Legacy: includes full bodies. Prefer `format_skills_index_for_prompt()`.
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut lines: Vec<String> = vec!["## Available Skills".to_string()];

    for skill in skills {
        if let Some(ref desc) = skill.description {
            lines.push(format!("- **{}**: {}", skill.name, desc));
        } else {
            lines.push(format!("- **{}**", skill.name));
        }
    }

    lines.push(String::new());
    for skill in skills {
        lines.push(format!("### {}", skill.name));
        lines.push(skill.body.clone());
        lines.push(String::new());
    }

    lines.join("\n")
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Discover skills from a single directory, parsing only frontmatter.
fn discover_skills_from_dir(dir: PathBuf, source: SkillSource) -> Result<Vec<SkillIndex>> {
    let mut indexes = Vec::new();

    if !dir.exists() || !dir.is_dir() {
        return Ok(indexes);
    }

    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("reading skills dir {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }

        if let Some(idx) = parse_index_only(&path, source.clone()) {
            indexes.push(idx);
        }
    }

    Ok(indexes)
}

/// Parse only the YAML frontmatter from a skill directory — skip the body.
///
/// Returns `None` if the SKILL.md can't be read or has no frontmatter.
fn parse_index_only(dir: &Path, source: SkillSource) -> Option<SkillIndex> {
    let skill_md = dir.join("SKILL.md");
    let raw = std::fs::read_to_string(&skill_md).ok()?;

    let (frontmatter, _body) = extract_frontmatter(&raw);

    let name = frontmatter
        .name
        .unwrap_or_else(|| dir.file_name().unwrap().to_string_lossy().to_string());

    Some(SkillIndex {
        name,
        description: frontmatter.description,
        source,
        directory: dir.to_path_buf(),
        requires: frontmatter.requires,
        disable_model_invocation: frontmatter.disable_model_invocation.unwrap_or(false),
    })
}

/// Extract YAML frontmatter and body from raw markdown.
fn extract_frontmatter(raw: &str) -> (SkillFrontmatter, String) {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        return (SkillFrontmatter::default(), raw.to_string());
    }

    let without_opening = &trimmed[3..];
    if let Some(end_idx) = without_opening.find("\n---") {
        let yaml_str = &without_opening[..end_idx];
        let body = without_opening[end_idx + 4..].trim_start().to_string();
        let frontmatter: SkillFrontmatter =
            serde_yaml::from_str(yaml_str).unwrap_or_default();
        (frontmatter, body)
    } else {
        (SkillFrontmatter::default(), raw.to_string())
    }
}

/// Extract the environment variable name from a template like `${VAR}` or `${VAR:-default}`.
fn extract_env_var_name(template: &str) -> String {
    let s = template.trim();
    if s.starts_with("${") && s.ends_with('}') {
        let inner = &s[2..s.len() - 1];
        // Handle ${VAR:-default} syntax
        if let Some(colon_idx) = inner.find(":-") {
            inner[..colon_idx].to_string()
        } else {
            inner.to_string()
        }
    } else {
        s.to_string()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Legacy tests (backward compat) ──────────────────────────────────

    #[test]
    fn load_skills_from_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills").join("review");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("SKILL.md"),
            "---\nname: code-review\ndescription: Review code\n---\n\n# Review\n\nUse this to review.",
        )
        .unwrap();

        let skills = load_skills(dir.path(), None).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "code-review");
        assert_eq!(skills[0].description.as_deref(), Some("Review code"));
        assert!(skills[0].body.contains("Use this to review"));
    }

    #[test]
    fn load_skills_channel_overrides_workspace() {
        let ws = tempfile::tempdir().unwrap();
        let ch = tempfile::tempdir().unwrap();

        let ws_skill = ws.path().join("skills").join("review");
        std::fs::create_dir_all(&ws_skill).unwrap();
        std::fs::write(ws_skill.join("SKILL.md"), "# Workspace review").unwrap();

        let ch_skill = ch.path().join("skills").join("review");
        std::fs::create_dir_all(&ch_skill).unwrap();
        std::fs::write(ch_skill.join("SKILL.md"), "# Channel review").unwrap();

        let skills = load_skills(ws.path(), Some(ch.path())).unwrap();
        assert_eq!(skills.len(), 1);
        // Channel should win.
        assert!(skills[0].body.contains("Channel review"));
    }

    #[test]
    fn format_skills_for_prompt_includes_bodies() {
        let skill = Skill {
            name: "code-review".into(),
            description: Some("Review code".into()),
            body: "# Review\n\nInstructions here.".into(),
            source: SkillSource::Workspace,
            directory: "/tmp/skills/review".into(),
        };

        let formatted = format_skills_for_prompt(&[skill]);
        assert!(formatted.contains("code-review"));
        assert!(formatted.contains("Review code"));
        assert!(formatted.contains("Instructions here"));
    }

    // ── TASK-29.1: Extended frontmatter tests ──────────────────────────

    #[test]
    fn frontmatter_parses_requires_bins_and_env() {
        let yaml = r#"---
name: vinted-scraper
description: Search Vinted
requires:
  bins: [curl, jq]
  env:
    GITHUB_TOKEN: "${GITHUB_TOKEN}"
---

# Body here"#;
        let (fm, body) = extract_frontmatter(yaml);
        assert_eq!(fm.name.as_deref(), Some("vinted-scraper"));
        assert_eq!(fm.description.as_deref(), Some("Search Vinted"));
        let requires = fm.requires.unwrap();
        assert_eq!(requires.bins.as_deref(), Some(&["curl".into(), "jq".into()][..]));
        assert_eq!(
            requires.env.as_ref().unwrap().get("GITHUB_TOKEN").map(|s| s.as_str()),
            Some("${GITHUB_TOKEN}")
        );
        assert!(body.contains("Body here"));
    }

    #[test]
    fn frontmatter_parses_disable_model_invocation() {
        let yaml = r#"---
name: admin-tool
description: Admin utility
disable-model-invocation: true
---

# Admin body"#;
        let (fm, _body) = extract_frontmatter(yaml);
        assert_eq!(fm.name.as_deref(), Some("admin-tool"));
        assert_eq!(fm.disable_model_invocation, Some(true));
    }

    #[test]
    fn frontmatter_without_new_fields_is_backward_compat() {
        let yaml = r#"---
name: code-review
description: Review code
---

# Body"#;
        let (fm, _body) = extract_frontmatter(yaml);
        assert_eq!(fm.name.as_deref(), Some("code-review"));
        assert!(fm.requires.is_none());
        assert!(fm.disable_model_invocation.is_none());
    }

    #[test]
    fn frontmatter_with_requires_bins_only() {
        let yaml = r#"---
name: tool
requires:
  bins: [git]
---

# Body"#;
        let (fm, _body) = extract_frontmatter(yaml);
        let requires = fm.requires.unwrap();
        assert_eq!(requires.bins.unwrap(), vec!["git"]);
        assert!(requires.env.is_none());
    }

    // ── TASK-29.2: SkillIndex discovery tests ───────────────────────────

    #[test]
    fn discover_skills_returns_only_metadata_no_body() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills").join("review");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("SKILL.md"),
            "---\nname: code-review\ndescription: Review code\n---\n\n# Long body that should NOT be loaded\n\nThis is a very long body with lots of instructions.",
        )
        .unwrap();

        let indexes = discover_skills(dir.path(), None).unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].name, "code-review");
        assert_eq!(indexes[0].description.as_deref(), Some("Review code"));
        // Metadata should NOT contain the body
        assert_eq!(indexes[0].source, SkillSource::Workspace);
    }

    #[test]
    fn discover_skills_channel_shadows_workspace() {
        let ws = tempfile::tempdir().unwrap();
        let ch = tempfile::tempdir().unwrap();

        let ws_skill = ws.path().join("skills").join("review");
        std::fs::create_dir_all(&ws_skill).unwrap();
        std::fs::write(
            ws_skill.join("SKILL.md"),
            "---\nname: code-review\ndescription: WS version\n---\n\n# WS body",
        )
        .unwrap();

        let ch_skill = ch.path().join("skills").join("review");
        std::fs::create_dir_all(&ch_skill).unwrap();
        std::fs::write(
            ch_skill.join("SKILL.md"),
            "---\nname: code-review\ndescription: CH version\n---\n\n# CH body",
        )
        .unwrap();

        let indexes = discover_skills(ws.path(), Some(ch.path())).unwrap();
        assert_eq!(indexes.len(), 1);
        // Channel should win
        assert_eq!(indexes[0].description.as_deref(), Some("CH version"));
        assert_eq!(indexes[0].source, SkillSource::Channel);
    }

    #[test]
    fn load_skill_body_resolves_base_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills").join("review");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("SKILL.md"),
            "---\nname: review\n---\n\nRun from {baseDir}",
        )
        .unwrap();

        let indexes = discover_skills(dir.path(), None).unwrap();
        assert_eq!(indexes.len(), 1);

        let skill = load_skill_body(&indexes[0]).unwrap();
        assert!(skill.body.contains(&skills_dir.to_string_lossy().to_string()));
        assert!(!skill.body.contains("{baseDir}"));
    }

    #[test]
    fn discover_skills_returns_empty_for_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let indexes = discover_skills(dir.path(), None).unwrap();
        assert!(indexes.is_empty());
    }

    #[test]
    fn parse_index_only_reads_requires_and_disable_flag() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills").join("scraper");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("SKILL.md"),
            r#"---
name: vinted-scraper
description: Search Vinted
requires:
  bins: [curl, jq]
disable-model-invocation: true
---

# Scraper body
"#,
        )
        .unwrap();

        let indexes = discover_skills(dir.path(), None).unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].name, "vinted-scraper");
        assert!(indexes[0].disable_model_invocation);
        let requires = indexes[0].requires.as_ref().unwrap();
        assert_eq!(requires.bins.as_deref(), Some(&["curl".into(), "jq".into()][..]));
    }

    // ── TASK-29.4: Environment gating tests ─────────────────────────────

    #[test]
    fn gate_skill_no_requires_is_always_available() {
        let idx = SkillIndex {
            name: "test".into(),
            description: None,
            source: SkillSource::Workspace,
            directory: "/tmp/skills/test".into(),
            requires: None,
            disable_model_invocation: false,
        };
        let gate = gate_skill(&idx);
        assert!(gate.available);
        assert!(gate.missing_bins.is_empty());
        assert!(gate.missing_env.is_empty());
    }

    #[test]
    fn gate_skill_missing_bin_is_unavailable() {
        let idx = SkillIndex {
            name: "test".into(),
            description: None,
            source: SkillSource::Workspace,
            directory: "/tmp/skills/test".into(),
            requires: Some(SkillRequires {
                bins: Some(vec!["nonexistent-bin-xyz-12345".into()]),
                env: None,
            }),
            disable_model_invocation: false,
        };
        let gate = gate_skill(&idx);
        assert!(!gate.available);
        assert_eq!(gate.missing_bins, vec!["nonexistent-bin-xyz-12345"]);
        assert!(gate.missing_env.is_empty());
    }

    #[test]
    fn gate_skill_existing_bin_is_available() {
        // `echo` and `ls` should always be available on Linux/macOS
        let idx = SkillIndex {
            name: "test".into(),
            description: None,
            source: SkillSource::Workspace,
            directory: "/tmp/skills/test".into(),
            requires: Some(SkillRequires {
                bins: Some(vec!["echo".into(), "ls".into()]),
                env: None,
            }),
            disable_model_invocation: false,
        };
        let gate = gate_skill(&idx);
        assert!(gate.available, "echo and ls should be on PATH");
        assert!(gate.missing_bins.is_empty());
    }

    #[test]
    fn gate_skill_home_env_is_available() {
        // HOME is always set on Unix
        let idx = SkillIndex {
            name: "test".into(),
            description: None,
            source: SkillSource::Workspace,
            directory: "/tmp/skills/test".into(),
            requires: Some(SkillRequires {
                bins: None,
                env: Some(HashMap::from([(
                    "HOME_VAR".into(),
                    "${HOME}".into(),
                )])),
            }),
            disable_model_invocation: false,
        };
        let gate = gate_skill(&idx);
        assert!(gate.available, "HOME should always be set");
    }

    #[test]
    fn gate_skill_missing_env_is_unavailable() {
        let idx = SkillIndex {
            name: "test".into(),
            description: None,
            source: SkillSource::Workspace,
            directory: "/tmp/skills/test".into(),
            requires: Some(SkillRequires {
                bins: None,
                env: Some(HashMap::from([(
                    "SECRET".into(),
                    "${MISSING_ENV_VAR_XYZ_12345}".into(),
                )])),
            }),
            disable_model_invocation: false,
        };
        let gate = gate_skill(&idx);
        assert!(!gate.available);
        assert_eq!(gate.missing_env, vec!["SECRET"]);
    }

    #[test]
    fn gate_skill_env_with_default_syntax() {
        // ${VAR:-default} should extract just VAR
        let idx = SkillIndex {
            name: "test".into(),
            description: None,
            source: SkillSource::Workspace,
            directory: "/tmp/skills/test".into(),
            requires: Some(SkillRequires {
                bins: None,
                env: Some(HashMap::from([(
                    "TOKEN".into(),
                    "${GITHUB_TOKEN:-fallback}".into(),
                )])),
            }),
            disable_model_invocation: false,
        };
        let gate = gate_skill(&idx);
        // GITHUB_TOKEN is likely not set, so it should be unavailable
        assert!(!gate.available);
        assert_eq!(gate.missing_env, vec!["TOKEN"]);
    }

    // ── TASK-29.3: format_skills_index_for_prompt tests ────────────────

    #[test]
    fn format_index_empty_returns_empty_string() {
        assert_eq!(format_skills_index_for_prompt(&[]), "");
    }

    #[test]
    fn format_index_includes_names_and_descriptions() {
        let idx = SkillIndex {
            name: "code-review".into(),
            description: Some("Review code".into()),
            source: SkillSource::Workspace,
            directory: "/tmp/skills/review".into(),
            requires: None,
            disable_model_invocation: false,
        };

        let formatted = format_skills_index_for_prompt(&[idx]);
        assert!(formatted.contains("code-review"));
        assert!(formatted.contains("Review code"));
        // Should include usage instruction
        assert!(formatted.contains("use `read` to load"));
    }

    #[test]
    fn format_index_excludes_disabled_skills() {
        let idx = SkillIndex {
            name: "admin".into(),
            description: Some("Admin tasks".into()),
            source: SkillSource::Workspace,
            directory: "/tmp/skills/admin".into(),
            requires: None,
            disable_model_invocation: true,
        };

        let formatted = format_skills_index_for_prompt(&[idx]);
        assert!(!formatted.contains("admin"));
    }

    #[test]
    fn format_index_marks_unavailable_skills() {
        let idx = SkillIndex {
            name: "scraper".into(),
            description: Some("Scrape sites".into()),
            source: SkillSource::Workspace,
            directory: "/tmp/skills/scraper".into(),
            requires: Some(SkillRequires {
                bins: Some(vec!["nonexistent-12345".into()]),
                env: None,
            }),
            disable_model_invocation: false,
        };

        let formatted = format_skills_index_for_prompt(&[idx]);
        assert!(formatted.contains("scraper"));
        assert!(formatted.contains("⚠️ unavailable"));
        assert!(formatted.contains("nonexistent-12345"));
    }

    #[test]
    fn format_index_output_is_compact() {
        // Verify the index doesn't include bodies (unlike format_skills_for_prompt)
        let idx = SkillIndex {
            name: "review".into(),
            description: Some("Review code".into()),
            source: SkillSource::Workspace,
            directory: "/tmp/skills/review".into(),
            requires: None,
            disable_model_invocation: false,
        };

        let formatted = format_skills_index_for_prompt(&[idx]);
        // Should NOT contain "###" headers (body sections)
        assert!(!formatted.contains("### "));
        // Should be small (< 500 chars for one skill)
        assert!(formatted.len() < 500);
    }
}
