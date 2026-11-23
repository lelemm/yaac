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
