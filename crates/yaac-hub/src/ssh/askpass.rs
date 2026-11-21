//! Getting a password or a key passphrase to `ssh` without a terminal and without a file
//! (docs/02-architecture.md#setting-up-an-ssh-connection: "used once and never stored").
//!
//! The hub starts `ssh` with `SSH_ASKPASS` = this program, `SSH_ASKPASS_REQUIRE=force`, and
//! two variables of its own: `YAAC_ASKPASS` (a local socket the hub just made: a Unix socket
//! in a fresh user-only directory, or a Windows named pipe whose only access-control entry is
//! the current user) and `YAAC_ASKPASS_TOKEN` (random). When `ssh` needs the secret it runs
//! the helper, which connects, sends the token and the prompt, and prints what it gets back.
//!
//! The hub answers **once**: the secret is moved out of its slot on the first valid request,
//! written to the socket, and wiped; a second prompt (a wrong password) gets nothing, so
//! `ssh` fails instead of asking again. Yes/no questions are never answered with the secret.
//! The socket and its directory are removed as soon as the session's sign-in is over.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use zeroize::Zeroizing;

/// The helper's socket, in its environment.
pub const ENV_SOCKET: &str = "YAAC_ASKPASS";
/// The helper's token, in its environment.
pub const ENV_TOKEN: &str = "YAAC_ASKPASS_TOKEN";

/// Random hex, from the system's generator on Unix.
pub(crate) fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    #[cfg(unix)]
    {
        use std::io::Read;
        if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).is_ok() {
            return buf.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
    // Windows (and a Unix without /dev/urandom): per-process random hasher keys, mixed with
    // time and a counter. The pipe is user-only either way; the token is a second lock.
    for chunk in buf.chunks_mut(8) {
        let id = crate::settings::new_id("");
        let n = u64::from_str_radix(&id[..16.min(id.len())], 16).unwrap_or(0).to_le_bytes();
        let len = chunk.len();
        chunk.copy_from_slice(&n[..len]);
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether a prompt is a question `ssh` asks (a new host key, a confirmation), which must not
/// be answered with the secret.
fn is_question(prompt: &str) -> bool {
    let p = prompt.to_ascii_lowercase();
    p.contains("yes/no") || p.contains("(yes/") || p.contains("continue connecting") || p.contains("fingerprint")
}

/// A one-shot askpass server. Dropping it stops it and removes its socket.
pub(crate) struct Askpass {
    socket: PathBuf,
    token: String,
    secret: Arc<Mutex<Option<Zeroizing<String>>>>,
    task: tokio::task::JoinHandle<()>,
    #[cfg_attr(windows, allow(dead_code))]
    dir: Option<PathBuf>,
}

impl std::fmt::Debug for Askpass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Askpass").field("socket", &self.socket).finish_non_exhaustive()
    }
}

impl Drop for Askpass {
    fn drop(&mut self) {
        self.task.abort();
        if let Ok(mut s) = self.secret.lock() {
            s.take();
        }
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

impl Askpass {
    /// The environment for `ssh`.
    pub(crate) fn env(&self, helper: &Path) -> Vec<(String, std::ffi::OsString)> {
        vec![
            ("SSH_ASKPASS".into(), helper.as_os_str().to_owned()),
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            // Older clients use askpass only with a display set.
            ("DISPLAY".into(), std::env::var_os("DISPLAY").unwrap_or_else(|| "yaac:0".into())),
            (ENV_SOCKET.into(), self.socket.as_os_str().to_owned()),
            (ENV_TOKEN.into(), self.token.clone().into()),
        ]
    }

    /// Whether the secret was handed out.
    pub(crate) fn used(&self) -> bool {
        self.secret.lock().is_ok_and(|s| s.is_none())
    }

    /// Serves `secret` once. `dir` is a private directory for the socket (Unix).
    #[cfg(unix)]
    pub(crate) fn start(secret: Zeroizing<String>, dir: &Path) -> std::io::Result<Self> {
        let mut base = dir.join(format!("askpass-{}", random_hex(6)));
        if base.join("s").as_os_str().len() > crate::paths::MAX_SOCKET_PATH {
            base = std::env::temp_dir().join(format!("yaac-askpass-{}", random_hex(6)));
        }
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base)?;
        super::private_dir(&base)?;
        let socket = base.join("s");
        let listener = tokio::net::UnixListener::bind(&socket)?;
        crate::paths::make_private_file(&socket)?;
        let token = random_hex(16);
        let slot = Arc::new(Mutex::new(Some(secret)));
        let (t, s) = (token.clone(), slot.clone());
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                answer(stream, &t, &s).await;
            }
        });
        Ok(Self { socket, token, secret: slot, task, dir: Some(base) })
    }

    /// Serves `secret` once, on a user-only named pipe.
    #[cfg(windows)]
    pub(crate) fn start(secret: Zeroizing<String>, _dir: &Path) -> std::io::Result<Self> {
        let socket = PathBuf::from(format!(r"\\.\pipe\yaac-askpass-{}", random_hex(12)));
        let first = yaac_winsys::create_pipe_server(socket.as_os_str(), true)?;
        let token = random_hex(16);
        let slot = Arc::new(Mutex::new(Some(secret)));
        let (t, s, name) = (token.clone(), slot.clone(), socket.clone());
        let task = tokio::spawn(async move {
            let mut server = first;
            loop {
                if server.connect().await.is_err() {
                    break;
                }
                let next = yaac_winsys::create_pipe_server(name.as_os_str(), false);
                answer(server, &t, &s).await;
                match next {
                    Ok(n) => server = n,
                    Err(_) => break,
                }
            }
        });
        Ok(Self { socket, token, secret: slot, task, dir: None })
    }
}

async fn answer<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(mut stream: S, token: &str, slot: &Mutex<Option<Zeroizing<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut req = Vec::new();
    let mut buf = [0u8; 1024];
    let read = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while req.iter().filter(|&&b| b == b'\n').count() < 2 && req.len() < 4096 {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => req.extend_from_slice(&buf[..n]),
            }
        }
    });
    if read.await.is_err() {
        return;
    }
    let text = String::from_utf8_lossy(&req);
    let mut lines = text.split('\n');
    if lines.next() != Some(token) {
        return;
    }
    if is_question(lines.next().unwrap_or("")) {
        return;
    }
    let Some(secret) = slot.lock().ok().and_then(|mut s| s.take()) else { return };
    let mut out = Zeroizing::new(Vec::with_capacity(secret.len() + 1));
    out.extend_from_slice(secret.as_bytes());
    out.push(b'\n');
    drop(secret);
    let _ = stream.write_all(&out).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// The askpass helper: when this process was started by `ssh` as `SSH_ASKPASS` (the
/// `YAAC_ASKPASS` variable is set), ask the hub for the secret, print it, and return the exit
/// code. `None` when this process is not the helper. Call it first thing in `main`.
pub fn client_main() -> Option<std::process::ExitCode> {
    let socket = std::env::var_os(ENV_SOCKET).filter(|s| !s.is_empty())?;
    let token = std::env::var(ENV_TOKEN).unwrap_or_default();
    let prompt = std::env::args().nth(1).unwrap_or_default().replace('\n', " ");
    Some(match ask(Path::new(&socket), &token, &prompt) {
        Some(secret) => {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            if out.write_all(&secret).and_then(|()| out.flush()).is_ok() { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
        }
        None => std::process::ExitCode::FAILURE,
    })
}

fn ask(socket: &Path, token: &str, prompt: &str) -> Option<Zeroizing<Vec<u8>>> {
    use std::io::{Read, Write};
    #[cfg(unix)]
    let mut stream = std::os::unix::net::UnixStream::connect(socket).ok()?;
    #[cfg(windows)]
    let mut stream = std::fs::OpenOptions::new().read(true).write(true).open(socket).ok()?;
    #[cfg(not(any(unix, windows)))]
    let mut stream: std::fs::File = {
        let _ = socket;
        return None;
    };
    stream.write_all(format!("{token}\n{prompt}\n").as_bytes()).ok()?;
    stream.flush().ok()?;
    let mut secret = Zeroizing::new(Vec::new());
    stream.read_to_end(&mut secret).ok()?;
    (!secret.is_empty()).then_some(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_never_get_the_secret() {
        assert!(is_question("Are you sure you want to continue connecting (yes/no/[fingerprint])? "));
        assert!(!is_question("dev@devbox's password: "));
        assert!(!is_question("Enter passphrase for key '/home/me/.ssh/id_rsa': "));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn the_secret_is_given_once_to_the_right_token() {
        let dir = std::env::temp_dir().join(format!("yaac-askpass-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let server = Askpass::start(Zeroizing::new("hunter2".to_owned()), &dir).unwrap();
        let socket = server.socket.clone();
        let token = server.token.clone();
        let (s, t) = (socket.clone(), token.clone());
        let wrong = tokio::task::spawn_blocking(move || ask(&s, "not-the-token", "password: ")).await.unwrap();
        assert!(wrong.is_none(), "a wrong token gets nothing");
        let (s, t2) = (socket.clone(), t.clone());
        let q = tokio::task::spawn_blocking(move || ask(&s, &t2, "Are you sure you want to continue connecting (yes/no)?")).await.unwrap();
        assert!(q.is_none() && !server.used(), "a question gets nothing and does not use the secret");
        let s = socket.clone();
        let got = tokio::task::spawn_blocking(move || ask(&s, &t, "password: ")).await.unwrap();
        assert_eq!(got.as_deref().map(Vec::as_slice), Some(&b"hunter2\n"[..]));
        assert!(server.used());
        let again = tokio::task::spawn_blocking(move || ask(&socket, &token, "password: ")).await.unwrap();
        assert!(again.is_none(), "only once");
        let base = server.dir.clone().unwrap();
        drop(server);
        assert!(!base.exists(), "the socket is removed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
