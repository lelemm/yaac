//! The side panes' data in the demo (scene [`super::Scene::SidePanes`]): the `api` worktree of
//! users-service as `fs/tree`, `fs/read` and `git/fileDiff` answer it, and the stored files
//! `upload/read` returns (a tool's screenshot, the published report). Every git mark appears
//! (M, A, D, R, U, !), with changed folders, an ignored folder and file, a binary file and one
//! over the read limit; the diffs have several hunks with long unchanged runs between them.

use serde_json::{Value, json};

/// The screenshot a tool call returned (`ToolImage`).
pub(super) const LOGIN_PNG: &[u8] = include_bytes!("assets/login.png");
pub(super) const LOGIN_UPLOAD: &str = "u_demo_login_png";
/// The published report and image (`GeneratedArtifact`): content-addressed ids, as `yaacd`
/// makes them (`u_artifact_<ext>_<sha256>`).
pub(super) const REPORT_MD: &[u8] = include_bytes!("assets/auth-migration-report.md");
pub(super) const REPORT_UPLOAD: &str = "u_artifact_md_037a80c4f654f1b33e1f0ec5e8b58c3247ce69235575a3bef44f86c956540dbf";
pub(super) const LOGIN_ARTIFACT: &str = "u_artifact_png_3d7d5fc59ae48cf450860f5d6fa8a7c6dc0d70529ad3c4962ea0dbd46225d1c9";

/// The demo machines' dev server (`hub/preview/tunnel`, `preview/ports`).
pub(super) const DEV_PORT: u64 = 5173;

/// The dev server's answer to one HTTP request: a page with something to click and type in
/// (for the browser and its agent tools), its script, or 404. Every answer closes.
pub(super) fn dev_server(request: &[u8]) -> Vec<u8> {
    let line = request.split(|b| *b == b'\r').next().unwrap_or_default();
    let path = std::str::from_utf8(line).ok().and_then(|l| l.split(' ').nth(1)).unwrap_or("/");
    let (status, kind, body) = match path.split('?').next().unwrap_or("/") {
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", DEV_PAGE),
        "/app.js" => ("200 OK", "text/javascript", DEV_SCRIPT),
        _ => ("404 Not Found", "text/plain", "Not found"),
    };
    format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}", body.len()).into_bytes()
}

const DEV_PAGE: &str = "<!doctype html><html><head><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>Sign in · api</title>\
<style>body{font:16px system-ui;margin:0;display:grid;place-items:center;min-height:100vh;background:#1e222a;color:#e6e9ef}\
form{background:#eceff4;color:#222;padding:24px;border-radius:12px;width:260px;display:grid;gap:12px}\
input,button{font:inherit;padding:8px;border-radius:6px;border:1px solid #bbb}button{background:#4070e0;color:#fff;border:0}</style></head>\
<body><form id=f><h2>Sign in</h2><input id=email placeholder=Email><input id=password type=password placeholder=Password>\
<button id=go type=submit>Sign in</button><p id=out></p></form><script src=/app.js></script></body></html>";

const DEV_SCRIPT: &str = "document.getElementById('f').addEventListener('submit',e=>{e.preventDefault();\
const who=document.getElementById('email').value||'nobody';console.log('signed in',who);\
document.getElementById('out').textContent='Signed in as '+who;});";

/// A file over the read limit (a database dump).
const DUMP_SIZE: u64 = 12 * 1024 * 1024;

/// An `upload/read` answer for a demo upload, all at once.
pub(super) fn upload(id: &str) -> Value {
    let (bytes, media) = match id {
        LOGIN_UPLOAD | LOGIN_ARTIFACT => (LOGIN_PNG, "image/png"),
        REPORT_UPLOAD => (REPORT_MD, "text/markdown"),
        _ => return json!({ "present": false }),
    };
    json!({ "present": true, "dataBase64": base64(bytes), "size": bytes.len(), "mediaType": media, "more": false })
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= c.len() { char::from(A[(n >> shift & 63) as usize]) } else { '=' });
        }
    }
    out
}

fn dir(name: &str, changed: bool) -> Value {
    json!({ "name": name, "kind": "dir", "ignored": false, "changed": changed })
}

fn file(name: &str, size: u64, git: Option<&str>) -> Value {
    let mut v = json!({ "name": name, "kind": "file", "size": size, "mtime": 1_795_000_000_000u64, "ignored": false, "changed": false });
    if let Some(g) = git {
        v["git"] = json!(g);
    }
    v
}

fn ignored(mut v: Value) -> Value {
    v["ignored"] = json!(true);
    v
}

/// `fs/tree` for `path` (relative to the worktree root). `None`: no such folder.
pub(super) fn tree(path: &str) -> Option<Value> {
    let len = |p: &str| text(p).map_or(0, |t| t.len() as u64);
    let entries = match path.trim_matches('/') {
        "" => vec![
            dir("crates", false),
            dir("docs", true),
            dir("migrations", true),
            dir("services", true),
            ignored(dir("target", false)),
            ignored(file(".env", 212, None)),
            file("Cargo.toml", len("Cargo.toml"), None),
            file("dump.sql", DUMP_SIZE, None),
            file("notes.txt", len("notes.txt"), Some("U")),
            file("README.md", len("README.md"), Some("M")),
        ],
        "crates" => vec![dir("auth", false)],
        "crates/auth" => vec![dir("src", false), file("Cargo.toml", 180, None)],
        "crates/auth/src" => vec![file("lib.rs", len("crates/auth/src/lib.rs"), None)],
        "docs" => vec![file("architecture.png", LOGIN_PNG.len() as u64, None), file("design.md", len("docs/design.md"), Some("M"))],
        "migrations" => vec![
            file("0006_init.sql", len("migrations/0006_init.sql"), None),
            file("0007_users_tokens.sql", len("migrations/0007_users_tokens.sql"), Some("!")),
        ],
        "services" => vec![dir("billing", true), dir("users", true)],
        "services/billing" => vec![dir("src", true)],
        "services/billing/src" => vec![file("client.rs", 2400, Some("M")), file("session.rs", 0, Some("D"))],
        "services/users" => vec![dir("src", true), file("Cargo.toml", len("services/users/Cargo.toml"), None)],
        "services/users/src" => vec![
            file("auth_client.rs", len("services/users/src/auth_client.rs"), Some("R")),
            file("handlers.rs", len("services/users/src/handlers.rs"), Some("M")),
            file("legacy.rs", 0, Some("D")),
            file("lib.rs", len("services/users/src/lib.rs"), Some("M")),
            file("schema.rs", len("services/users/src/schema.rs"), Some("A")),
            file("store.rs", len("services/users/src/store.rs"), None),
        ],
        "target" => vec![ignored(dir("debug", false))],
        _ => return None,
    };
    Some(json!({ "entries": entries, "repo": true, "truncated": false }))
}

/// `fs/read` for `path`: text, a binary file, or one over `max_bytes`.
pub(super) fn read(path: &str, max_bytes: u64) -> Option<Value> {
    let path = path.trim_matches('/');
    if path == "docs/architecture.png" {
        return Some(json!({ "binary": true, "size": LOGIN_PNG.len(), "truncated": false, "etag": "demo-png" }));
    }
    if path == "dump.sql" {
        return Some(json!({ "size": DUMP_SIZE, "truncated": DUMP_SIZE > max_bytes, "etag": "demo-dump" }));
    }
    let t = text(path)?;
    Some(json!({ "text": t, "size": t.len(), "truncated": false, "etag": format!("demo-{}", t.len()) }))
}

/// `git/fileDiff` for `path`: both sides, the mark, and a rename's old path.
pub(super) fn diff(path: &str) -> Option<Value> {
    let path = path.trim_matches('/');
    let (old, new, status, old_path): (Option<String>, Option<String>, &str, Option<&str>) = match path {
        "services/users/src/handlers.rs" => (Some(handlers(false)), Some(handlers(true)), "M", None),
        "services/users/src/auth_client.rs" => (Some(LEGACY.to_owned()), text(path), "R", Some("services/users/src/legacy.rs")),
        "services/users/src/legacy.rs" => (Some(LEGACY.to_owned()), None, "D", None),
        "services/users/src/schema.rs" => (None, text(path), "A", None),
        "services/users/src/lib.rs" => (Some(LIB_OLD.to_owned()), text(path), "M", None),
        "README.md" => (Some(README_OLD.to_owned()), text(path), "M", None),
        "docs/design.md" => (Some(DESIGN_OLD.to_owned()), text(path), "M", None),
        "migrations/0007_users_tokens.sql" => (Some(MIGRATION_OURS.to_owned()), text(path), "!", None),
        "notes.txt" => (None, text(path), "U", None),
        _ => return None,
    };
    let mut v = json!({ "status": status, "binary": false, "etag": format!("demo-diff-{path}") });
    if let Some(o) = old {
        v["old"] = json!(o);
    }
    if let Some(n) = new {
        v["new"] = json!(n);
    }
    if let Some(p) = old_path {
        v["oldPath"] = json!(p);
    }
    Some(v)
}

/// The working copy's text of a file in the tree.
fn text(path: &str) -> Option<String> {
    Some(match path {
        "Cargo.toml" => "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\", \"services/*\"]\n".into(),
        "README.md" => README_NEW.into(),
        "notes.txt" => "- ask search-service for the index ETA\n- remove legacy/auth after the rebuild\n".into(),
        "crates/auth/src/lib.rs" => "//! The new auth module.\npub mod token;\n\npub use token::{Client, Token};\n".into(),
        "docs/design.md" => DESIGN_NEW.into(),
        "migrations/0006_init.sql" => "CREATE TABLE users (\n    id BIGINT PRIMARY KEY,\n    email TEXT NOT NULL UNIQUE\n);\n".into(),
        "migrations/0007_users_tokens.sql" => MIGRATION_CONFLICT.into(),
        "services/users/Cargo.toml" => "[package]\nname = \"users\"\nversion = \"0.3.0\"\nedition = \"2021\"\n\n[dependencies]\nauth = { path = \"../../crates/auth\" }\naxum = \"0.8\"\n".into(),
        "services/users/src/auth_client.rs" => LEGACY.replace("use legacy_auth::Session;", "use auth::Token;").replace("session_from", "bearer"),
        "services/users/src/handlers.rs" => handlers(true),
        "services/users/src/lib.rs" => LIB_NEW.into(),
        "services/users/src/schema.rs" => "//! The users table as the new auth module sees it.\nuse serde::{Deserialize, Serialize};\n\n#[derive(Debug, Serialize, Deserialize)]\npub struct User {\n    pub id: i64,\n    pub email: String,\n    pub subject: String,\n}\n".into(),
        "services/users/src/store.rs" => "use crate::schema::User;\n\npub struct Store;\n\nimpl Store {\n    pub async fn user(&self, id: i64) -> Result<User, crate::Error> {\n        todo_user(id)\n    }\n}\n".into(),
        _ => return None,
    })
}

/// A long handler file; the working copy changes three places far apart, so the diff has
/// three hunks with folded unchanged runs between them.
fn handlers(new: bool) -> String {
    let mut s = String::from("//! HTTP handlers for users-service.\nuse axum::extract::{Path, State};\nuse axum::http::HeaderMap;\nuse axum::Json;\n\n");
    s.push_str(if new { "use crate::auth_client;\n" } else { "use crate::legacy;\n" });
    s.push_str("use crate::{AppState, Error, User};\n\n");
    s.push_str("pub async fn me(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<User>, Error> {\n");
    if new {
        s.push_str("    let token = auth_client::bearer(&headers)?;\n    let id = s.auth.verify(&token).await?.subject;\n");
    } else {
        s.push_str("    let session = legacy::session_from(&headers)?;\n    let id = session.user_id();\n");
    }
    s.push_str("    let user = s.store.user(id).await?;\n    Ok(Json(user))\n}\n\n");
    for name in ["list", "get", "create", "update", "deactivate", "reactivate", "search", "export"] {
        s.push_str(&format!(
            "/// `{name}`: see docs/api.md#{name}.\npub async fn {name}(State(s): State<AppState>, Path(id): Path<i64>) -> Result<Json<User>, Error> {{\n    let user = s.store.user(id).await?;\n    tracing::debug!(user = user.id, \"{name}\");\n    Ok(Json(user))\n}}\n\n"
        ));
    }
    s.push_str("pub async fn delete(State(s): State<AppState>, Path(id): Path<i64>) -> Result<(), Error> {\n");
    if new {
        s.push_str("    s.auth.revoke_all(id).await?;\n");
    }
    s.push_str("    s.store.delete(id).await\n}\n");
    if new {
        s.push_str("\n/// Tokens issued to this user, newest first.\npub async fn tokens(State(s): State<AppState>, Path(id): Path<i64>) -> Result<Json<Vec<String>>, Error> {\n    Ok(Json(s.auth.tokens_of(id).await?))\n}\n");
    }
    s
}

const LEGACY: &str = "use legacy_auth::Session;\nuse axum::http::HeaderMap;\n\nuse crate::Error;\n\npub fn session_from(headers: &HeaderMap) -> Result<Session, Error> {\n    let raw = headers.get(\"authorization\").ok_or(Error::Unauthorized)?;\n    Session::parse(raw.to_str()?).map_err(|_| Error::Unauthorized)\n}\n";
const LIB_OLD: &str = "mod handlers;\nmod store;\nmod legacy;\n\npub use handlers::router;\n";
const LIB_NEW: &str = "mod handlers;\nmod store;\nmod auth_client;\nmod schema;\n\npub use handlers::router;\npub use schema::User;\n";
const README_OLD: &str = "# api\n\nThe services behind the product.\n\nAuthentication uses `legacy/auth`.\n";
const README_NEW: &str = "# api\n\nThe services behind the product.\n\nAuthentication uses `crates/auth` (the migration is tracked in docs/design.md).\n";
const DESIGN_OLD: &str = "# Design\n\n## Auth\n\nSessions are cookies checked by `legacy/auth`.\n";
const DESIGN_NEW: &str = "# Design\n\n## Auth\n\nBearer tokens verified by `crates/auth`.\n\n```mermaid\nsequenceDiagram\n    client->>users: GET /me (Bearer)\n    users->>auth: verify(token)\n    auth-->>users: subject\n```\n";
const MIGRATION_OURS: &str = "ALTER TABLE users ADD COLUMN subject TEXT;\n";
const MIGRATION_CONFLICT: &str = "<<<<<<< HEAD\nALTER TABLE users ADD COLUMN subject TEXT;\n=======\nALTER TABLE users ADD COLUMN subject TEXT NOT NULL DEFAULT '';\n>>>>>>> billing-service\n";

#[cfg(test)]
mod tests {
    #[test]
    fn every_mark_and_case_is_served() {
        let root = super::tree("").unwrap();
        let entries = root["entries"].as_array().unwrap();
        assert!(entries.iter().any(|e| e["ignored"] == true && e["kind"] == "file"));
        assert!(entries.iter().any(|e| e["ignored"] == true && e["kind"] == "dir"));
        assert!(entries.iter().any(|e| e["changed"] == true));
        let src = super::tree("services/users/src").unwrap();
        let marks: Vec<_> = src["entries"].as_array().unwrap().iter().filter_map(|e| e["git"].as_str()).collect();
        for m in ["R", "M", "D", "A"] {
            assert!(marks.contains(&m), "{m}");
        }
        assert_eq!(super::read("docs/architecture.png", 1 << 22).unwrap()["binary"], true);
        assert_eq!(super::read("dump.sql", 4 << 20).unwrap()["truncated"], true);
        assert!(super::read("services/users/src/handlers.rs", 1 << 22).unwrap()["text"].is_string());
        let rename = super::diff("services/users/src/auth_client.rs").unwrap();
        assert_eq!(rename["oldPath"], "services/users/src/legacy.rs");
        let d = super::diff("services/users/src/handlers.rs").unwrap();
        assert_ne!(d["old"], d["new"]);
        assert_eq!(super::diff("migrations/0007_users_tokens.sql").unwrap()["status"], "!");
        assert_eq!(super::upload(super::LOGIN_UPLOAD)["present"], true);
        assert_eq!(super::upload("nope")["present"], false);
        assert_eq!(super::base64(b"Man"), "TWFu");
        assert_eq!(super::base64(b"Ma"), "TWE=");
        assert_eq!(super::base64(b"M"), "TQ==");
        let page = String::from_utf8(super::dev_server(b"GET / HTTP/1.1\r\nHost: localhost:5173\r\n\r\n")).unwrap();
        assert!(page.starts_with("HTTP/1.1 200 OK") && page.contains("Sign in"));
        assert!(String::from_utf8(super::dev_server(b"GET /nope HTTP/1.1\r\n\r\n")).unwrap().contains("404"));
    }
}
