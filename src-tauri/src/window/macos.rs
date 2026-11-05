//! macOS overlay behaviour (AppKit):
//! - the overlay floats above other apps, follows you to every Space and shows over
//!   full-screen apps (Zoom/Teams/Keynote in full screen), and stays out of Cmd+` cycling;
//! - typing mode remembers which app was in front and hands focus back to it afterwards.

use std::sync::atomic::{AtomicBool, Ordering};

use objc2_app_kit::{
    NSApplicationActivationOptions, NSNormalWindowLevel, NSRunningApplication, NSStatusWindowLevel,
    NSWindow, NSWindowCollectionBehavior, NSWorkspace,
};
use tauri::{Runtime, WebviewWindow};

static TYPING: AtomicBool = AtomicBool::new(false);

pub fn set_typing(typing: bool) {
    TYPING.store(typing, Ordering::SeqCst);
}

pub fn is_typing() -> bool {
    TYPING.load(Ordering::SeqCst)
}

/// pid of the frontmost app (0 when unknown).
pub fn frontmost_pid() -> i32 {
    // SAFETY: plain AppKit getters; NSWorkspace is safe to use from any thread.
    unsafe {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|a| a.processIdentifier())
            .unwrap_or(0)
    }
}

/// Whether `pid` is this app.
pub fn is_own_pid(pid: i32) -> bool {
    pid > 0 && pid as u32 == std::process::id()
}

/// Brings the app with this pid back to the front. False if it's gone (or it's us).
pub fn activate_pid(pid: i32) -> bool {
    if pid <= 0 || is_own_pid(pid) {
        return false;
    }
    // SAFETY: plain AppKit calls on a retained NSRunningApplication.
    unsafe {
        match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            Some(app) => app.activateWithOptions(NSApplicationActivationOptions(0)),
            None => false,
        }
    }
}

/// Window level and Spaces behaviour for the overlay. Runs on the main thread (AppKit
/// requires it); safe to call from anywhere.
pub fn configure_overlay<R: Runtime>(w: &WebviewWindow<R>, on_top: bool) {
    let Ok(ptr) = w.ns_window() else {
        return;
    };
    let raw = ptr as usize;
    let _ = w.run_on_main_thread(move || {
        // SAFETY: `ns_window` is the live NSWindow of this webview window, and we're on the
        // main thread.
        let ns: &NSWindow = unsafe { &*(raw as *const NSWindow) };
        let behaviour = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle;
        // SAFETY: plain property setter on the main thread.
        unsafe { ns.setCollectionBehavior(behaviour) };
        // Status level keeps it above normal floating panels and over full-screen apps.
        ns.setLevel(if on_top { NSStatusWindowLevel } else { NSNormalWindowLevel });
    });
}
