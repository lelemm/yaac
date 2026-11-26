//! Skills: `skills/list` (docs/04-providers.md#skills).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{InstanceId, TargetPath};
use crate::macros::open_enum;

open_enum! {
    /// Where a skill comes from, as far as the harness says.
    pub enum SkillScope fallback Other {
        /// The user's own (`~/.claude/skills`, `~/.codex/skills`).
        User = "user",
        /// Belongs to the project the folder is in (`.claude/skills`, `.agents/skills`).
        Project = "project",
        /// Ships with the harness or a plugin.
        System = "system",
    }
}

/// `skills/list`: the skills one provider instance offers for a working directory. Scoped by
/// the target (the daemon answering), the instance (its harness home), and the folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillsListParams {
    /// The instance; needs its `skills` capability.
    pub instance: InstanceId,
    /// The working directory whose project skills are meant.
    pub cwd: TargetPath,
    /// Scan the disk again instead of answering from the harness's cache.
    #[serde(default)]
    pub force_reload: bool,
}

/// One skill.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    /// The name the user types (`/name`, `$name`).
    pub name: String,
    /// What it is for, one line.
    pub description: String,
    /// Where its `SKILL.md` is on the target.
    pub path: String,
    /// Where it comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<SkillScope>,
    /// Hint for its arguments ("[file]"), when the skill gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
}

/// `skills/list` result. Disabled skills are left out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillsListResult {
    /// The skills, by name.
    pub skills: Vec<SkillInfo>,
    /// Skills the harness could not read, one line each (for diagnostics; never fatal).
    #[serde(default)]
    pub errors: Vec<String>,
}

/// One skill owned by the user's YAAC installation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSkillInfo {
    /// Public identifier, such as `yaac.create-yaac-theme`.
    pub id: String,
    /// A short description for discovery.
    pub description: String,
    /// Offer it to the model for selection when relevant.
    pub automatic: bool,
    /// Immutable bundle revision.
    pub revision: String,
    /// The installer supplies this skill.
    pub bundled: bool,
}

/// A regular file of a catalog skill, relative to the root of the bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSkillFile {
    /// Portable relative path using slashes.
    pub path: String,
    /// The file's bytes.
    pub data_base64: String,
    /// Whether to preserve the owner's execute bit on Unix.
    #[serde(default)]
    pub executable: bool,
}

/// Complete bundle staged on a target on demand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSkill {
    /// Discovery metadata and version.
    pub info: CatalogSkillInfo,
    /// The entrypoint and supporting files.
    pub files: Vec<CatalogSkillFile>,
}

/// Read either discovery metadata or one full skill from the hub.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSkillsParams {
    /// Omit to list metadata. Include the catalog ID to retrieve its bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Metadata listing or a single immutable bundle.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSkillsResult {
    /// The metadata returned on list.
    #[serde(default)]
    pub skills: Vec<CatalogSkillInfo>,
    /// The bundle returned on retrieval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<CatalogSkill>,
}

/// A user edits a catalog entry in the attached window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSkillSave {
    /// Set by the window, also used as the stored revision.
    pub op_id: crate::OpId,
    /// The version the user edited, absent for a new entry.
    pub previous_revision: Option<String>,
    /// Skill data. The hub assigns the revision and bundled marker.
    pub skill: CatalogSkill,
}

/// The user removes their version of a skill from the catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSkillRemove {
    /// The operation identity.
    pub op_id: crate::OpId,
    /// The catalog ID.
    pub id: String,
    /// The version confirmed for removal.
    pub previous_revision: String,
}

/// Maximum decoded bytes in one transferable catalog bundle.
pub const MAX_CATALOG_SKILL_BYTES: usize = 512 * 1024;
