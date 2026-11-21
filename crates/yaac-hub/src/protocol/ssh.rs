//! The `hub/ssh/*` types (docs/02-architecture.md#setting-up-an-ssh-connection).

use serde::{Deserialize, Serialize};

/// An SSH server as the setup methods name it: *user@host:port*. It is the same server as an
/// SSH target whose transport has the same host, port (22 when absent), and user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConnParams {
    /// Host name or `~/.ssh/config` alias.
    pub host: String,
    /// Port; 22 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// User; the `ssh` default when absent or empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// `hub/ssh/reach` result: the server's host key, to confirm.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHostKey {
    /// `ED25519`, `ECDSA`, `RSA`.
    pub algorithm: String,
    /// `SHA256:…`, as `ssh-keygen -l` prints it.
    pub fingerprint: String,
}

/// `hub/ssh/probe` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshProbeParams {
    /// The server.
    #[serde(flatten)]
    pub conn: SshConnParams,
    /// The fingerprint the user confirmed; must be the one `hub/ssh/reach` returned.
    pub fingerprint: String,
}

/// `hub/ssh/probe` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshProbeResult {
    /// Signing in with no password worked: access was already in place, and YAAC installs
    /// nothing (`false`: sign in once with `hub/ssh/signIn`).
    pub in_place: bool,
}

/// `hub/ssh/signIn` params: exactly one of `password` and `keyFile`. Never logged; its
/// `Debug` masks the secrets, and the hub wipes them after their one use.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshSignInParams {
    /// The server.
    #[serde(flatten)]
    pub conn: SshConnParams,
    /// The password, used once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// A private key file on this computer, used for this one login (never copied).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_file: Option<String>,
    /// The key file's passphrase, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
}

impl std::fmt::Debug for SshSignInParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mask = |s: &Option<String>| s.as_ref().map(|_| "(hidden)");
        f.debug_struct("SshSignInParams")
            .field("conn", &self.conn)
            .field("password", &mask(&self.password))
            .field("key_file", &self.key_file)
            .field("passphrase", &mask(&self.passphrase))
            .finish()
    }
}

/// `hub/ssh/signIn` result: the line YAAC appended to `authorized_keys`, complete (the user
/// may have to remove it by hand).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshInstalledKey {
    /// `ssh-ed25519 AAAA… yaac:<machine>:<id>`.
    pub line: String,
    /// `yaac:<machine>:<id>`.
    pub comment: String,
}

/// `hub/ssh/revoke` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshRevokeResult {
    /// A line was removed (`false`: it was already gone, or already revoked).
    pub removed: bool,
}

/// What YAAC did on one server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SshRecordKind {
    /// YAAC installed its key and proved it works; Revoke removes it.
    InstalledByYaac,
    /// YAAC appended its key and the proof has not finished (Revoke still removes it).
    Installing,
    /// Password-less access already worked; YAAC installed nothing (Revoke is disabled).
    AlreadyInPlace,
    /// Revoke removed YAAC's key.
    Revoked,
    /// The user removed YAAC's line by hand; YAAC deleted its key.
    RemovedByHand,
    /// Setup did not finish; nothing of YAAC's is on the server.
    NotSetUp,
}

/// One server's record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshRecord {
    /// What happened.
    pub kind: SshRecordKind,
    /// The installed line (for `installedByYaac` and `installing`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// Its comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// When, milliseconds since the epoch.
    pub at_ms: i64,
}

impl SshRecord {
    /// YAAC has a line on the server that Revoke would remove.
    pub fn has_key(&self) -> bool {
        matches!(self.kind, SshRecordKind::InstalledByYaac | SshRecordKind::Installing)
    }
}

/// `hub/ssh/records` entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshRecordEntry {
    /// The server.
    #[serde(flatten)]
    pub conn: SshConnParams,
    /// What YAAC did there.
    pub record: SshRecord,
}

/// `hub/ssh/records` result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshRecordsResult {
    /// One per server YAAC ran Setup on.
    pub records: Vec<SshRecordEntry>,
}
