//! Durable catalog and immutable bundles. All access is on the hub's store worker.
use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use yaac_protocol::{
    CatalogSkill, CatalogSkillFile, CatalogSkillInfo, CatalogSkillRemove, CatalogSkillSave,
    CatalogSkillsParams, CatalogSkillsResult, MAX_CATALOG_SKILL_BYTES,
};

use crate::drafts::DraftStore;

const THEME: &str = include_str!("../../../catalog/skills/create-yaac-theme/SKILL.md");
const THEME_ID: &str = "yaac.create-yaac-theme";

fn built_in() -> CatalogSkill {
    let description = THEME
        .lines()
        .find_map(|line| line.strip_prefix("description: "))
        .unwrap_or_default();
    CatalogSkill {
        info: CatalogSkillInfo {
            id: THEME_ID.into(),
            description: description.into(),
            automatic: true,
            revision: format!("bundled-{:x}", Sha256::digest(THEME)),
            bundled: true,
        },
        files: vec![CatalogSkillFile {
            path: "SKILL.md".into(),
            data_base64: STANDARD.encode(THEME),
            executable: false,
        }],
    }
}

impl DraftStore {
    pub(crate) fn catalog(
        &self,
        request: &CatalogSkillsParams,
    ) -> Result<CatalogSkillsResult, String> {
        if let Some(id) = &request.id {
            let skill = self
                .catalog_skill(id)?
                .ok_or_else(|| "No skill with that catalog ID.".to_owned())?;
            return Ok(CatalogSkillsResult {
                skill: Some(skill),
                ..Default::default()
            });
        }
        let mut rows = BTreeMap::from([(THEME_ID.to_owned(), built_in().info)]);
        let mut stmt = self
            .conn
            .prepare("SELECT info FROM catalog_skills")
            .map_err(db)?;
        for info in stmt.query_map([], |r| r.get::<_, String>(0)).map_err(db)? {
            let info: CatalogSkillInfo = serde_json::from_str(&info.map_err(db)?).map_err(db)?;
            rows.insert(info.id.clone(), info);
        }
        Ok(CatalogSkillsResult {
            skills: rows.into_values().collect(),
            skill: None,
        })
    }

    fn catalog_skill(&self, id: &str) -> Result<Option<CatalogSkill>, String> {
        let row = self
            .conn
            .query_row(
                "SELECT info, files FROM catalog_skills WHERE id = ?1",
                [id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(db)?;
        match row {
            Some((info, files)) => Ok(Some(CatalogSkill {
                info: serde_json::from_str(&info).map_err(db)?,
                files: serde_json::from_str(&files).map_err(db)?,
            })),
            None => Ok((id == THEME_ID).then(built_in)),
        }
    }

    pub(crate) fn save_catalog_skill(
        &self,
        mut request: CatalogSkillSave,
    ) -> Result<CatalogSkill, String> {
        let skill = &mut request.skill;
        validate(skill)?;
        let op = request.op_id.as_str();
        if !safe_revision(op) {
            return Err("Invalid skill operation ID.".into());
        }
        skill.info.revision = op.to_owned();
        skill.info.bundled = false;
        let current = self.catalog_skill(&skill.info.id)?;
        if current.as_ref() == Some(skill) {
            return Ok(request.skill);
        }
        if current
            .as_ref()
            .is_some_and(|item| item.info.revision == op)
            || current.as_ref().map(|item| item.info.revision.as_str())
                != request.previous_revision.as_deref()
        {
            return Err("The skill changed. Reopen it before saving.".into());
        }
        if current.is_none() && self.catalog(&CatalogSkillsParams::default())?.skills.len() >= 32 {
            return Err("The catalog holds up to 32 skills.".into());
        }
        self.conn
            .execute(
                "INSERT OR REPLACE INTO catalog_skills (id, info, files) VALUES (?1, ?2, ?3)",
                params![
                    skill.info.id,
                    serde_json::to_string(&skill.info).map_err(db)?,
                    serde_json::to_string(&skill.files).map_err(db)?
                ],
            )
            .map_err(db)?;
        Ok(request.skill)
    }

    pub(crate) fn remove_catalog_skill(&self, request: &CatalogSkillRemove) -> Result<(), String> {
        if !safe_revision(request.op_id.as_str()) {
            return Err("Invalid skill operation ID.".into());
        }
        let current = self.catalog_skill(&request.id)?;
        if let Some(skill) = current {
            if skill.info.bundled || skill.info.revision != request.previous_revision {
                return Err("The skill changed or is bundled with YAAC.".into());
            }
            self.conn
                .execute("DELETE FROM catalog_skills WHERE id = ?1", [&request.id])
                .map_err(db)?;
        }
        Ok(())
    }
}

fn db(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn safe_revision(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 80
        && text
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}

fn validate(skill: &CatalogSkill) -> Result<(), String> {
    let slug = skill.info.id.strip_prefix("yaac.").unwrap_or("");
    if slug.is_empty()
        || slug.len() > 64
        || slug.split('-').any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
    {
        return Err(
            "Use a yaac. name followed by lowercase letters, numbers, or single hyphens.".into(),
        );
    }
    if skill.info.description.trim().is_empty()
        || skill.info.description.len() > 240
        || skill.info.description.chars().any(char::is_control)
    {
        return Err("Enter a description of 1–240 bytes on one line.".into());
    }
    if skill.files.is_empty() || skill.files.len() > 64 {
        return Err("A skill must have between 1 and 64 files.".into());
    }

    let mut paths = std::collections::BTreeSet::new();
    let mut size = 0;
    let mut entrypoint = false;
    for file in &skill.files {
        if !portable_path(&file.path) || !paths.insert(file.path.to_lowercase()) {
            return Err(format!(
                "Invalid or duplicate skill file path: {}",
                file.path
            ));
        }
        if file.data_base64.len() > MAX_CATALOG_SKILL_BYTES.div_ceil(3) * 4 {
            return Err("Skill files exceed 512 KiB.".into());
        }
        let bytes = STANDARD
            .decode(&file.data_base64)
            .map_err(|_| "Invalid skill file encoding.")?;
        size += bytes.len();
        if size > MAX_CATALOG_SKILL_BYTES {
            return Err("Skill files exceed 512 KiB.".into());
        }
        if file.path == "SKILL.md" {
            entrypoint = std::str::from_utf8(&bytes).is_ok_and(|s| !s.trim().is_empty());
        }
    }
    for name in &paths {
        if name
            .match_indices('/')
            .any(|(index, _)| paths.contains(&name[..index]))
        {
            return Err("A skill path is both a file and a folder.".into());
        }
    }
    if !entrypoint {
        return Err("Include a nonempty UTF-8 SKILL.md at the root.".into());
    }
    Ok(())
}

fn portable_path(path: &str) -> bool {
    path.len() <= 240
        && path.split('/').all(|part| {
            let stem = part
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            !(part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with(['.', ' '])
                || part
                    .chars()
                    .any(|c| c.is_control() || r#":"\<>|?*"#.contains(c))
                || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || ["COM", "LPT"].iter().any(|prefix| {
                    stem.strip_prefix(prefix).is_some_and(|n| {
                        n.chars().count() == 1
                            && n.chars().all(|c| matches!(c, '1'..='9' | '¹' | '²' | '³'))
                    })
                }))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_edits_are_durable_and_reject_stale_or_escaping_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drafts.sqlite");
        let store = DraftStore::open(&path).unwrap();
        let mut skill = built_in();
        skill.info.id = "yaac.custom".into();
        skill.info.automatic = false;
        let mut request = CatalogSkillSave {
            skill,
            op_id: yaac_protocol::OpId::new("new"),
            previous_revision: None,
        };
        request.skill.files.push(CatalogSkillFile {
            path: "../malicious".into(),
            data_base64: "".into(),
            executable: false,
        });
        assert!(store.save_catalog_skill(request.clone()).is_err());
        request.skill.files.pop();
        let saved = store.save_catalog_skill(request.clone()).unwrap();
        assert_eq!(store.save_catalog_skill(request.clone()).unwrap(), saved);
        drop(store);
        let store = DraftStore::open(&path).unwrap();
        let listed = store
            .catalog(&CatalogSkillsParams::default())
            .unwrap()
            .skills;
        assert!(listed.contains(&saved.info));
        assert_eq!(
            store.catalog_skill("yaac.custom").unwrap(),
            Some(saved.clone())
        );
        request.op_id = yaac_protocol::OpId::new("update");
        assert!(store.save_catalog_skill(request.clone()).is_err());
        request.previous_revision = Some(saved.info.revision);
        assert!(store.save_catalog_skill(request).is_ok());
    }
}
