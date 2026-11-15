use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

pub struct SkillSrc {
    pub name: &'static str,
    pub description: &'static str,
    pub uri: &'static str,
    pub content: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillSummary {
    pub name: &'static str,
    pub description: &'static str,
    pub uri: &'static str,
    pub bytes: usize,
}

pub const SKILLS: &[SkillSrc] = &[
    SkillSrc {
        name: "buchcraft-advisor",
        description: "Austrian double-entry bookkeeping (EKR), Abschlussbuchungen, U30 VAT pre-fill, Storno, and account guidance in BuchCraft.",
        uri: "skill://buchcraft-advisor/SKILL.md",
        content: include_str!("../../../skills/buchcraft-advisor/SKILL.md"),
    },
    SkillSrc {
        name: "buchcraft-workflow",
        description: "Monthly and year-end workflows in BuchCraft: bank CSV import, AfA drafts, U30 check, and period closing.",
        uri: "skill://buchcraft-workflow/SKILL.md",
        content: include_str!("../../../skills/buchcraft-workflow/SKILL.md"),
    },
];

pub fn skills_list() -> Vec<SkillSummary> {
    SKILLS
        .iter()
        .map(|s| SkillSummary {
            name: s.name,
            description: s.description,
            uri: s.uri,
            bytes: s.content.len(),
        })
        .collect()
}

pub fn skill_read(name: &str) -> Result<Value, String> {
    let found = SKILLS
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("unknown skill '{name}'"))?;
    Ok(json!({
        "name": found.name,
        "uri": found.uri,
        "description": found.description,
        "content": found.content,
    }))
}

pub fn skill_by_uri(uri: &str) -> Option<&'static SkillSrc> {
    SKILLS.iter().find(|s| s.uri == uri)
}

pub fn mcp_resources() -> Vec<Value> {
    SKILLS
        .iter()
        .map(|s| {
            json!({
                "uri": s.uri,
                "name": s.name,
                "description": s.description,
                "mimeType": "text/markdown",
            })
        })
        .collect()
}

pub fn export_skills(dest_dir: &Path) -> Result<Vec<String>, String> {
    fs::create_dir_all(dest_dir).map_err(|e| format!("create dir failed: {e}"))?;
    let mut written = Vec::new();
    for s in SKILLS {
        let dir = dest_dir.join(s.name);
        fs::create_dir_all(&dir).map_err(|e| format!("create dir failed: {e}"))?;
        let path = dir.join("SKILL.md");
        fs::write(&path, s.content).map_err(|e| format!("write failed: {e}"))?;
        written.push(path.display().to_string());
    }
    Ok(written)
}
