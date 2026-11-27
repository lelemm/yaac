//! The Windows implementations. Each unsafe block states why it is sound.

use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::ptr;
use std::sync::OnceLock;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, GetCurrentProcess, INFINITE, OpenEventW, OpenProcessToken, SetEvent,
    SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow, SW_SHOWNORMAL};

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

fn win32(rc: u32) -> io::Result<()> {
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(i32::try_from(rc).unwrap_or(i32::MAX)))
    }
}

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Memory the system allocated with `LocalAlloc`; freed on drop.
struct Local(*mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from an API documented to allocate with LocalAlloc,
            // and it is freed exactly once.
            unsafe { LocalFree(self.0) };
        }
    }
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: an open handle this module owns, closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Attaches to the console of the process that started this one (a terminal), so a
/// GUI-subsystem `yaac.exe` started as `yaac hub` from `cmd` or PowerShell can print there.
/// Does nothing when standard output or error already go somewhere (a pipe, a file), or when
/// there is no parent console (started from Explorer). Returns whether it attached.
pub fn attach_parent_console() -> bool {
    let missing = |which| {
        // SAFETY: GetStdHandle has no preconditions; it only reads the process parameters.
        let h = unsafe { GetStdHandle(which) };
        h.is_null() || h == INVALID_HANDLE_VALUE
    };
    if !(missing(STD_OUTPUT_HANDLE) && missing(STD_ERROR_HANDLE)) {
        return false;
    }
    // SAFETY: no preconditions; fails harmlessly when there is no parent console.
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) != 0 }
}

/// Runs `f` with the current process's user SID (valid only during the call).
fn with_user_sid<T>(f: impl FnOnce(PSID) -> io::Result<T>) -> io::Result<T> {
    let mut raw: HANDLE = ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle; `raw` is a valid out pointer.
    check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut raw) })?;
    let token = Handle(raw);
    let mut len = 0u32;
    // SAFETY: a size query: a null buffer of length 0 with a valid out pointer for the size.
    unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &raw mut len) };
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    // u64 elements keep the buffer aligned for TOKEN_USER.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` is writable for at least `len` bytes.
    check(unsafe { GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr().cast(), len, &raw mut len) })?;
    // SAFETY: the call succeeded, so the buffer starts with an initialised, aligned TOKEN_USER
    // whose SID points into the same buffer, which outlives `f`.
    let user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
    f(user.User.Sid)
}

/// The current user's SID as text (`S-1-5-21-…`). Cached after the first call.
pub fn current_user_sid() -> io::Result<String> {
    static SID: OnceLock<Option<String>> = OnceLock::new();
    SID.get_or_init(|| {
        with_user_sid(|sid| {
            let mut text: *mut u16 = ptr::null_mut();
            // SAFETY: `sid` is valid for this call; `text` is a valid out pointer.
            check(unsafe { ConvertSidToStringSidW(sid, &raw mut text) })?;
            let _free = Local(text.cast());
            // SAFETY: the API returned a NUL-terminated wide string.
            let len = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
            // SAFETY: `len` u16s were just read from it.
            let slice = unsafe { std::slice::from_raw_parts(text, len) };
            Ok(String::from_utf16_lossy(slice))
        })
        .ok()
    })
    .clone()
    .ok_or_else(|| io::Error::other("cannot read the current user's SID"))
}

/// Creates one instance of a named pipe server that only the current user can open — the
/// owner and the only entry in a protected DACL are the user's SID — and that rejects remote
/// clients. `first` makes creation fail if any instance of the name already exists (another
/// process got there first). Must be called inside a tokio runtime.
pub fn create_pipe_server(name: &OsStr, first: bool) -> io::Result<NamedPipeServer> {
    let sid = current_user_sid()?;
    let sddl = wide(format!("O:{sid}D:P(A;;GA;;;{sid})"));
    let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `sddl` is NUL-terminated; `sd` is a valid out pointer; the size is optional.
    check(unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &raw mut sd, ptr::null_mut()) })?;
    let _free = Local(sd);
    let mut attrs = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    let mut options = ServerOptions::new();
    options.first_pipe_instance(first).reject_remote_clients(true);
    // SAFETY: `attrs` is a valid SECURITY_ATTRIBUTES whose descriptor lives until `_free`
    // drops, after this call returns.
    unsafe { options.create_with_security_attributes_raw(name, (&raw mut attrs).cast()) }
}

/// Whether the kernel object behind `object` (an opened pipe) is owned by the current user.
pub fn owned_by_current_user(object: &impl AsHandle) -> io::Result<bool> {
    let handle = object.as_handle().as_raw_handle();
    let mut owner: PSID = ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `handle` is borrowed from `object`, so it stays open for the call; the
    // out pointers are valid; the unused ones are null as the API allows.
    win32(unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut sd,
        )
    })?;
    let _free = Local(sd);
    // SAFETY: `owner` points into `sd`, alive until `_free` drops; `me` is valid in the closure.
    with_user_sid(|me| Ok(unsafe { EqualSid(owner, me) } != 0))
}

/// Sets a string value under `HKEY_CURRENT_USER\<subkey>`, creating the key if needed.
pub fn registry_set_string(subkey: &str, name: &str, value: &OsStr) -> io::Result<()> {
    let data = wide(value);
    let bytes = u32::try_from(data.len() * 2).map_err(|_| io::Error::other("value too long"))?;
    let (subkey, name) = (wide(subkey), wide(name));
    // SAFETY: all strings are NUL-terminated; `data` is readable for `bytes` bytes.
    win32(unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr(), REG_SZ, data.as_ptr().cast(), bytes) })
}

/// Reads a string value under `HKEY_CURRENT_USER\<subkey>`; `None` when the key or value is
/// absent.
pub fn registry_get_string(subkey: &str, name: &str) -> io::Result<Option<OsString>> {
    let (subkey, name) = (wide(subkey), wide(name));
    let mut len = 0u32;
    // SAFETY: a size query: null data pointer, valid size out pointer.
    let rc = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, ptr::null_mut(), ptr::null_mut(), &raw mut len)
    };
    if rc == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    win32(rc)?;
    let mut buf = vec![0u16; (len as usize).div_ceil(2) + 1];
    let mut len = u32::try_from(buf.len() * 2).unwrap_or(u32::MAX);
    // SAFETY: `buf` is writable for `len` bytes.
    let rc = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, ptr::null_mut(), buf.as_mut_ptr().cast(), &raw mut len)
    };
    if rc == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    win32(rc)?;
    buf.truncate(len as usize / 2);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    Ok(Some(OsString::from_wide(&buf)))
}

/// Deletes a value under `HKEY_CURRENT_USER\<subkey>`; absent is fine.
pub fn registry_delete_value(subkey: &str, name: &str) -> io::Result<()> {
    let (subkey, name) = (wide(subkey), wide(name));
    // SAFETY: both strings are NUL-terminated.
    let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr()) };
    if rc == ERROR_FILE_NOT_FOUND { Ok(()) } else { win32(rc) }
}

/// Deletes `HKEY_CURRENT_USER\<subkey>` with everything under it (tests' throwaway keys);
/// absent is fine. Refuses an empty subkey.
pub fn registry_delete_tree(subkey: &str) -> io::Result<()> {
    if subkey.trim_matches('\\').is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "refusing to delete the root of HKEY_CURRENT_USER"));
    }
    let subkey = wide(subkey);
    // SAFETY: the string is NUL-terminated.
    let rc = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, subkey.as_ptr()) };
    if rc == ERROR_FILE_NOT_FOUND { Ok(()) } else { win32(rc) }
}

/// Opens an `http(s)` URL in the user's default browser (`ShellExecuteW`, verb `open`). Blocks
/// for as long as the shell takes: call it from a worker thread. Anything but `http://` and
/// `https://` is refused, so a URL can never name a program.
pub fn open_url(url: &str) -> io::Result<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(io::Error::other("only http and https URLs are opened"));
    }
    shell_open(url)
}

/// Open a user-selected snapshot through its registered file association.
pub fn open_document(path: &std::path::Path) -> io::Result<()> {
    if !path.is_absolute() || !path.is_file() { return Err(io::Error::other("missing document")); }
    shell_open(path.to_str().ok_or_else(|| io::Error::other("invalid document path"))?)
}

fn shell_open(value: &str) -> io::Result<()> {
    let (verb, target) = (wide("open"), wide(value));
    // SAFETY: both strings are NUL-terminated and outlive the call; no window handle, no
    // parameters, no directory.
    let rc = unsafe { ShellExecuteW(ptr::null_mut(), verb.as_ptr(), target.as_ptr(), ptr::null(), ptr::null(), SW_SHOWNORMAL) };
    // ShellExecuteW returns a value greater than 32 on success.
    if rc as usize > 32 { Ok(()) } else { Err(io::Error::other(format!("ShellExecuteW failed ({})", rc as usize))) }
}

/// The window's single-instance claim (D127): a named auto-reset event in the session's
/// `Local\` namespace. The first window creates it and waits on it; a second `yaac` finds it,
/// signals it (the first window shows itself), and exits.
#[derive(Debug)]
pub struct WindowInstance(Handle);

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Handle")
    }
}

/// [`claim_window_instance`]'s outcome.
#[derive(Debug)]
pub enum InstanceClaim {
    /// This is the only window: keep the claim for as long as the window runs.
    First(WindowInstance),
    /// Another window holds the name; it was asked to show itself.
    Signalled,
}

/// Claims `name` (`Local\yaac-window-…`) for this window, or, when another window holds it,
/// asks that window to show itself and lets it take the foreground.
pub fn claim_window_instance(name: &str) -> io::Result<InstanceClaim> {
    let w = wide(name);
    // SAFETY: default security, auto-reset, not signalled, NUL-terminated name.
    let raw = unsafe { CreateEventW(ptr::null(), 0, 0, w.as_ptr()) };
    // SAFETY: no preconditions; read right after the call it describes.
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let event = Handle(raw);
    if !existed {
        return Ok(InstanceClaim::First(WindowInstance(event)));
    }
    // The running window is a background process: let it come to the front, since the user
    // just asked for it by starting this one.
    allow_window_foreground();
    // SAFETY: an open event handle owned here.
    check(unsafe { SetEvent(event.0) })?;
    Ok(InstanceClaim::Signalled)
}

/// Transfer foreground permission following an explicit Show or notification Open.
pub(crate) fn allow_window_foreground() {
    // SAFETY: no preconditions; failure only means the foreground stays where it is.
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
}

impl WindowInstance {
    /// Blocks until another `yaac` asks this window to show itself. Returns `false` if the
    /// wait failed (the caller stops waiting). Call it on a thread of its own.
    pub fn wait(&self) -> bool {
        // SAFETY: an open event handle owned by `self`, which outlives the call.
        unsafe { WaitForSingleObject(self.0.0, INFINITE) == WAIT_OBJECT_0 }
    }
}

// SAFETY: an event handle may be waited on and closed from any thread.
unsafe impl Send for WindowInstance {}
// SAFETY: waiting on an event from several threads at once is allowed.
unsafe impl Sync for WindowInstance {}

/// Creates (or opens) the named auto-reset event `name` (`Local\\…`) for this process to wait
/// on with [`WindowInstance::wait`]: the window's "quit" request from `yaac quit`.
pub fn listen_named_event(name: &str) -> io::Result<WindowInstance> {
    let w = wide(name);
    // SAFETY: default security, auto-reset, not signalled, NUL-terminated name.
    let raw = unsafe { CreateEventW(ptr::null(), 0, 0, w.as_ptr()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(WindowInstance(Handle(raw)))
}

/// Signals the named event `name` if some process has created it. `Ok(false)`: nobody has
/// (never creates it).
pub fn signal_named_event(name: &str) -> io::Result<bool> {
    let w = wide(name);
    // SAFETY: NUL-terminated name; a null handle is checked below.
    let raw = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, w.as_ptr()) };
    if raw.is_null() {
        let e = io::Error::last_os_error();
        return if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) { Ok(false) } else { Err(e) };
    }
    let event = Handle(raw);
    // SAFETY: an open event handle with EVENT_MODIFY_STATE, owned here.
    check(unsafe { SetEvent(event.0) })?;
    Ok(true)
}

/// Whether some process holds the named event `name` (never creates it).
pub fn named_event_exists(name: &str) -> bool {
    let w = wide(name);
    // SAFETY: NUL-terminated name; a null handle is checked below.
    let raw = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, w.as_ptr()) };
    if raw.is_null() {
        return false;
    }
    drop(Handle(raw));
    true
}


/// The user's first preferred Windows display language as a locale name (`pt-BR`), or `None`.
pub fn user_ui_language() -> Option<String> {
    use windows_sys::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    let mut count = 0u32;
    let mut len = 0u32;
    // SAFETY: a null buffer with a zero length asks only for the needed length, written to `len`.
    let ok = unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, ptr::null_mut(), &mut len) };
    if ok == 0 || len == 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: `buf` holds `len` UTF-16 units, the length the previous call asked for.
    let ok = unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buf.as_mut_ptr(), &mut len) };
    if ok == 0 {
        return None;
    }
    // A double-NUL-terminated list; the first entry is the one the user prefers.
    let first = buf.split(|&c| c == 0).next().unwrap_or_default();
    let name = String::from_utf16_lossy(first);
    (!name.is_empty()).then_some(name)
}
