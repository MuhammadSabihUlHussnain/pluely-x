//! Win32 helpers for the overlay window.
//!
//! tao rewrites the whole `GWL_EXSTYLE` on every flag change (show, hide, focusable,
//! click-through, always-on-top) and re-adds `WS_EX_APPWINDOW`, so a one-time
//! `WS_EX_TOOLWINDOW` patch doesn't stick. A `SetWindowSubclass` hook (which runs before
//! tao's own subclass) fixes the style on every `WM_STYLECHANGING`, refuses mouse
//! activation, and strips activation from `SetWindowPos`/`ShowWindow` while the overlay is
//! not in typing mode. See docs/SPIKE_WINDOW.md for the measurements behind this.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowLongPtrW, IsWindow, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST, MA_NOACTIVATE, STYLESTRUCT, SWP_ASYNCWINDOWPOS,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER,
    WINDOWPOS, WM_MOUSEACTIVATE, WM_NCDESTROY, WM_STYLECHANGING, WM_WINDOWPOSCHANGING,
    WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

const SUBCLASS_ID: usize = 0x504C_5558; // "PLUX"

/// True while the overlay is in typing mode (it may take activation and focus).
static TYPING: AtomicBool = AtomicBool::new(false);

pub fn set_typing(typing: bool) {
    TYPING.store(typing, Ordering::SeqCst);
}

pub fn is_typing() -> bool {
    TYPING.load(Ordering::SeqCst)
}

#[inline]
pub fn hwnd_from_isize(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
}

#[inline]
pub fn hwnd_to_isize(hwnd: HWND) -> isize {
    hwnd.0 as isize
}

unsafe extern "system" fn overlay_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _ref_data: usize,
) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE if !is_typing() => {
            return LRESULT(MA_NOACTIVATE as isize);
        }
        WM_STYLECHANGING if wparam.0 as i32 == GWL_EXSTYLE.0 && lparam.0 != 0 => {
            let ss = &mut *(lparam.0 as *mut STYLESTRUCT);
            ss.styleNew = (ss.styleNew | WS_EX_TOOLWINDOW.0) & !WS_EX_APPWINDOW.0;
        }
        WM_WINDOWPOSCHANGING if !is_typing() && lparam.0 != 0 => {
            let pos = &mut *(lparam.0 as *mut WINDOWPOS);
            pos.flags |= SWP_NOACTIVATE;
        }
        WM_NCDESTROY => {
            let _ = RemoveWindowSubclass(hwnd, Some(overlay_subclass_proc), SUBCLASS_ID);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// Install the overlay subclass and force the tool-window style. Must run on the thread that
/// owns the window (the tao main thread).
pub fn install_overlay_subclass(hwnd: HWND) -> bool {
    let ok = unsafe { SetWindowSubclass(hwnd, Some(overlay_subclass_proc), SUBCLASS_ID, 0) }.as_bool();
    force_tool_window(hwnd);
    ok
}

/// Add WS_EX_TOOLWINDOW / remove WS_EX_APPWINDOW right now (the hook keeps it that way).
pub fn force_tool_window(hwnd: HWND) {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let want = (ex | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
        if want != ex {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
}

/// Re-assert HWND_TOPMOST without activating (other topmost windows can cover the overlay).
/// Safe to call from any thread; uses SWP_ASYNCWINDOWPOS so it never blocks on the UI thread.
pub fn ensure_topmost(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS,
        );
    }
}

pub fn ex_style(hwnd: HWND) -> isize {
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) }
}

/// Flags relevant to "is this window in Alt+Tab / non-activating".
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExStyleFlags {
    pub raw: isize,
    pub tool_window: bool,
    pub app_window: bool,
    pub no_activate: bool,
}

pub fn ex_style_flags(hwnd: HWND) -> ExStyleFlags {
    let raw = ex_style(hwnd);
    ExStyleFlags {
        raw,
        tool_window: raw & WS_EX_TOOLWINDOW.0 as isize != 0,
        app_window: raw & WS_EX_APPWINDOW.0 as isize != 0,
        no_activate: raw & WS_EX_NOACTIVATE.0 as isize != 0,
    }
}

pub fn foreground_window() -> isize {
    unsafe { hwnd_to_isize(GetForegroundWindow()) }
}

/// Give the foreground back to `raw` if it is still a window. Returns whether it worked.
pub fn restore_foreground(raw: isize) -> bool {
    if raw == 0 {
        return false;
    }
    let hwnd = hwnd_from_isize(raw);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        SetForegroundWindow(hwnd).as_bool()
    }
}
