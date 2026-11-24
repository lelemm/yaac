//! Keep the caller's caption when the DevTools frontend updates its own document title.
use cef::*;

pub(super) fn apply(browser: &mut Browser, title: &str) -> bool {
    let Some(host) = browser.host() else {
        return false;
    };
    let handle = host.window_handle();
    #[cfg(windows)]
    let missing = handle.0.is_null();
    #[cfg(target_os = "linux")]
    let missing = handle == 0;
    if missing {
        return false;
    }
    native_title(handle, title);
    true
}

#[cfg(target_os = "linux")]
fn native_title(handle: WindowHandle, title: &str) {
    let Ok(window) = u32::try_from(handle) else {
        return;
    };
    let title = title.to_owned();
    std::thread::spawn(move || {
        let _ = keep_x11_title(window, &title);
    });
}

#[cfg(target_os = "linux")]
fn keep_x11_title(window: u32, title: &str) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::{
        connection::Connection,
        protocol::{
            Event,
            xproto::{ChangeWindowAttributesAux, ConnectionExt, EventMask, PropMode},
        },
        wrapper::ConnectionExt as _,
    };
    let (conn, _) = x11rb::connect(None)?;
    let name = conn.intern_atom(false, b"_NET_WM_NAME")?.reply()?.atom;
    let utf8 = conn.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
    conn.change_window_attributes(
        window,
        &ChangeWindowAttributesAux::new()
            .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
    )?
    .check()?;
    let set = || -> Result<(), Box<dyn std::error::Error>> {
        let current = conn
            .get_property(false, window, name, utf8, 0, 2048)?
            .reply()?;
        if current.bytes_after != 0 || current.value != title.as_bytes() {
            conn.change_property8(PropMode::REPLACE, window, name, utf8, title.as_bytes())?
                .check()?;
        }
        Ok(())
    };
    set()?;
    loop {
        match conn.wait_for_event()? {
            Event::PropertyNotify(event) if event.atom == name => set()?,
            Event::DestroyNotify(_) => return Ok(()),
            _ => {}
        }
    }
}

#[cfg(windows)]
fn native_title(handle: WindowHandle, title: &str) {
    use windows_sys::Win32::{
        Foundation::HWND,
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{GA_ROOT, GetAncestor, SetWindowTextW, WM_NCDESTROY, WM_SETTEXT},
        },
    };
    // The subclass belongs exclusively to this CEF DevTools window. It observes
    // caption changes and releases the backing UTF-16 string on window destruction.
    unsafe extern "system" fn caption(
        hwnd: HWND,
        msg: u32,
        w: usize,
        l: isize,
        id: usize,
        data: usize,
    ) -> isize {
        // SAFETY: data is the Box<Vec<u16>> installed below. Both installation and
        // WM_NCDESTROY run on the window's own thread.
        unsafe {
            if msg == WM_SETTEXT {
                return DefSubclassProc(
                    hwnd,
                    msg,
                    w,
                    (*(data as *const Vec<u16>)).as_ptr() as isize,
                );
            }
            if msg == WM_NCDESTROY {
                RemoveWindowSubclass(hwnd, Some(caption), id);
                drop(Box::from_raw(data as *mut Vec<u16>));
            }
            DefSubclassProc(hwnd, msg, w, l)
        }
    }
    let title = Box::new(title.encode_utf16().chain(Some(0)).collect::<Vec<_>>());
    // SAFETY: CEF supplied a live native handle on the window's UI thread.
    unsafe {
        let hwnd = GetAncestor(handle.0.cast(), GA_ROOT);
        let ptr = Box::into_raw(title);
        if SetWindowSubclass(hwnd, Some(caption), 1, ptr as usize) != 0 {
            SetWindowTextW(hwnd, (*ptr).as_ptr());
        } else {
            drop(Box::from_raw(ptr));
        }
    }
}
