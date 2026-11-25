//! Read-only filesystem helpers for pickers and the Files pane.

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

open_enum! {
    /// An entry in the worktree browser.
    pub enum FsTreeEntryKind fallback Unknown {
        /// A directory.
        Dir = "dir",
        /// A regular file.
        File = "file",
        /// A symbolic link.
        Symlink = "symlink",
    }
}

open_enum! {
    /// The git mark for a worktree file.
    pub enum GitFileStatus fallback Unknown {
        /// Modified.
        Modified = "M",
        /// Added to the index.
        Added = "A",
        /// Deleted.
        Deleted = "D",
        /// Renamed.
        Renamed = "R",
        /// Untracked.
        Untracked = "U",
        /// Unmerged.
        Conflict = "!",
    }
}

/// `fs/tree`: list one directory in a worktree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsTreeParams {
    /// Absolute worktree root on the target.
    pub root: TargetPath,
    /// Path relative to the root, empty for the root itself.
    pub path: TargetPath,
}

/// One child in the worktree browser, including a deleted path if tracked by git.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsTreeEntry {
    /// Base name relative to the listed directory.
    pub name: String,
    /// Entry kind.
    pub kind: FsTreeEntryKind,
    /// File size in bytes when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Last modified time in milliseconds since the Unix epoch, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime: Option<u64>,
    /// Git mark for this entry, if changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitFileStatus>,
    /// Git ignores this entry.
    pub ignored: bool,
    /// This directory contains a changed entry.
    pub changed: bool,
}

/// `fs/tree` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsTreeResult {
    /// Directories first, then files and links, each group sorted by name.
    pub entries: Vec<FsTreeEntry>,
    /// The worktree has a git repository.
    pub repo: bool,
    /// The listing was cut short.
    pub truncated: bool,
}

/// `fs/read`: read a file in a worktree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsReadParams {
    /// Absolute worktree root on the target.
    pub root: TargetPath,
    /// Path relative to the root.
    pub path: TargetPath,
    /// Maximum file size to return, in bytes, subject to the daemon's limits.
    pub max_bytes: u64,
    /// Raw byte offset returned by the previous page; absent for a first page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
}

/// `fs/read` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FsReadResult {
    /// This page of decoded text when the file is neither binary nor over the limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A NUL occurs in the first 8 KiB.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[schemars(schema_with = "binary_marker_schema")]
    pub binary: bool,
    /// Full file size in bytes.
    pub size: u64,
    /// The file exceeds the applicable byte limit.
    pub truncated: bool,
    /// Raw byte offset for the next page, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u64>,
    /// Opaque file revision; a changed value invalidates previously read pages.
    pub etag: String,
}

fn binary_marker_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "type": "boolean", "const": true })
}

/// `git/fileDiff`: compare the working copy with HEAD.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GitFileDiffParams {
    /// Absolute worktree root on the target.
    pub root: TargetPath,
    /// Path relative to the root, using the destination name of a rename.
    pub path: TargetPath,
    /// Raw byte offset for HEAD text. Omit both offsets to request the first page of each side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_offset: Option<u64>,
    /// Raw byte offset for working-copy text. On subsequent pages, omit an exhausted side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_offset: Option<u64>,
}

/// `git/fileDiff` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GitFileDiffResult {
    /// This page of HEAD text, if requested, present and both versions are textual.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<String>,
    /// This page of working-copy text, if requested, present and both versions are textual.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<String>,
    /// Previous path for a rename, relative to the worktree root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<TargetPath>,
    /// Git mark for this file.
    pub status: GitFileStatus,
    /// At least one version is binary.
    pub binary: bool,
    /// Raw byte offset for the next page of HEAD text, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_old_offset: Option<u64>,
    /// Raw byte offset for the next page of working-copy text, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_new_offset: Option<u64>,
    /// Opaque diff revision; a changed value invalidates previously read pages.
    pub etag: String,
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
