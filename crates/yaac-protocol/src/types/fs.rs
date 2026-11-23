//! Read-only filesystem helpers for pickers: `fs/list`, `project/resolve`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ProjectRef, TargetPath, WorktreeRef};
use crate::macros::open_enum;

open_enum! {
    /// What a directory entry is.
    pub enum FsEntryKind fallback Unknown {
        /// A directory.
        Directory = "directory",
        /// A regular file.
        File = "file",
        /// A symbolic link.
        Symlink = "symlink",
        /// Anything else.
        Other = "other",
    }
}

/// `fs/list`: list a directory on the target.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsListParams {
    /// The directory; the user's home when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<TargetPath>,
    /// Include hidden entries.
    #[serde(default)]
    pub include_hidden: bool,
}

/// One entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    /// File name.
    pub name: String,
    /// Full path, to pass back.
    pub path: TargetPath,
    /// What it is.
    pub kind: FsEntryKind,
}

/// `fs/list` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsListResult {
    /// The directory listed.
    pub path: TargetPath,
    /// Its parent, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<TargetPath>,
    /// Entries.
    pub entries: Vec<FsEntry>,
    /// The listing was cut short.
    #[serde(default)]
    pub truncated: bool,
}

/// `fs/search`: search for files relative to the working directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsSearchParams {
    /// Absolute working directory on the target.
    pub cwd: TargetPath,
    /// Case-insensitive path search.
    pub query: String,
}

/// A bounded set of matching files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsSearchResult {
    /// Files, with display names relative to the working directory and full target paths.
    pub entries: Vec<FsEntry>,
    /// More matches or entries may exist.
    pub truncated: bool,
}

/// `project/resolve`: the git identity of a directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectResolveParams {
    /// The directory.
    pub path: TargetPath,
}

/// `project/resolve` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectResolveResult {
    /// The project (the directory itself when there is no git).
    pub project: ProjectRef,
    /// The worktree; absent when there is no git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeRef>,
    /// Git top-level directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_root: Option<TargetPath>,
    /// Git common directory (shared by all worktrees).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub common_dir: Option<TargetPath>,
    /// Checked-out branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// `origin` remote URL, as configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_url: Option<String>,
}
