//! SKILL.md loading for duga frontends.
//!
//! Skills are Markdown files with optional YAML frontmatter that provide
//! reusable instructions for coding agents. Skills are loaded from the
//! workspace root `skills/` directory and optional channel-level `skills/`.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A loaded skill with name, description, and body.
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

/// Internal YAML frontmatter for SKILL.md files.
#[derive(Default, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
}

/// Load skills from workspace-level and optional channel-level `skills/` directories.
///
/// Channel skills override workspace skills on name collision.
/// Supports `{baseDir}` placeholders which resolve to the skill's directory.
pub fn load_skills(
    workspace: &Path,
    channel_dir: Option<&Path>,
) -> Result<Vec<Skill>> {
    let mut map: HashMap<String, Skill> = HashMap::new();

    // Load workspace skills first.
    let ws_skills = load_skills_from_dir(workspace.join("skills"), SkillSource::Workspace)?;
    for skill in ws_skills {
        map.insert(skill.name.clone(), skill);
    }

    // Load channel skills (override on collision).
    if let Some(channel) = channel_dir {
        let ch_skills =
            load_skills_from_dir(channel.join("skills"), SkillSource::Channel)?;
        for skill in ch_skills {
            map.insert(skill.name.clone(), skill);
        }
    }

    Ok(map.into_values().collect())
}

fn load_skills_from_dir(dir: PathBuf, source: SkillSource) -> Result<Vec<Skill>> {
    let mut skills = Vec::new();

    if !dir.exists() || !dir.is_dir() {
        return Ok(skills);
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

        let raw = std::fs::read_to_string(&skill_md)
            .with_context(|| format!("reading {}", skill_md.display()))?;

        let skill = parse_skill_md(&raw, &path, source.clone());
        skills.push(skill);
    }

    Ok(skills)
}

fn parse_skill_md(raw: &str, dir: &Path, source: SkillSource) -> Skill {
    let (frontmatter, body) = extract_frontmatter(raw);
    let name = frontmatter
        .name
        .unwrap_or_else(|| dir.file_name().unwrap().to_string_lossy().to_string());

    // Resolve {baseDir} placeholders.
    let body = body.replace("{baseDir}", &dir.to_string_lossy());

    Skill {
        name,
        description: frontmatter.description,
        body,
        source,
        directory: dir.to_path_buf(),
    }
}

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

/// Format loaded skills as a string for injection into the system prompt.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
