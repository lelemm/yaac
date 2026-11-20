//! Projects the user created (docs/05-threads.md#projects): a folder on the target with a name
//! the user gave it. Stored on the target, so every host sees the same ones.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{OpId, ProjectId, RepoLocation, TargetPath, Timestamp};

/// A project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// Stable id.
    pub id: ProjectId,
    /// The name the user gave it.
    pub name: String,
    /// Its folder, absolute on the target.
    pub folder: TargetPath,
    /// Where the folder sits in a git repository, when it is in one (noted when the project
    /// is created, refreshed when the target starts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<RepoLocation>,
    /// Normalised `origin` remote URL of that repository, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_target_key: Option<String>,
    /// Creation time.
    pub created_at: Timestamp,
}

/// `project/list`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListParams {}

/// `project/list` result, and `project/updated` params: every project on the target.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListResult {
    /// The projects, by name.
    #[serde(default)]
    pub projects: Vec<Project>,
}

/// `project/create`. Every thread whose folder falls inside the new project moves under it at
/// once (each is pushed as `thread/updated`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreateParams {
    /// Operation id.
    pub op_id: OpId,
    /// The folder; must exist and be a directory.
    pub folder: TargetPath,
    /// The name. Free text; need not match the folder.
    pub name: String,
}

/// `project/rename`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRenameParams {
    /// Operation id.
    pub op_id: OpId,
    /// The project.
    pub project_id: ProjectId,
    /// The new name.
    pub name: String,
}

/// `project/delete`. The folder and the threads are untouched; the threads fall back to an
/// outer project or a temporary one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDeleteParams {
    /// Operation id.
    pub op_id: OpId,
    /// The project.
    pub project_id: ProjectId,
}

/// Result of `project/create` and `project/rename`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectResult {
    /// The project after the change.
    pub project: Project,
}
