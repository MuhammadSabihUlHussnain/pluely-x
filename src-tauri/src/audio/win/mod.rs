//! WASAPI backend (Windows only).
//!
//! - [`com`]: per-thread MTA guard.
//! - [`devices`]: endpoint lookup and listing (call on an MTA thread).
//! - [`notify`]: `IMMNotificationClient` that only forwards events into a channel.
//! - [`control`]: the long-lived `audio-ctl` thread (listing + notifications).
//! - [`capture`]: per-source capture threads (`cap-me`, `cap-them`).

pub mod capture;
pub mod com;
pub mod control;
pub mod devices;
pub mod notify;

use windows::core::HRESULT;

/// `AUDCLNT_E_*` / `E_*` values we branch on.
pub mod hr {
    use windows::core::HRESULT;
    pub const E_ACCESSDENIED: HRESULT = HRESULT(0x8007_0005u32 as i32);
    pub const AUDCLNT_E_DEVICE_INVALIDATED: HRESULT = HRESULT(0x8889_0004u32 as i32);
    pub const AUDCLNT_E_UNSUPPORTED_FORMAT: HRESULT = HRESULT(0x8889_0008u32 as i32);
    pub const E_NOTFOUND: HRESULT = HRESULT(0x8007_0490u32 as i32);
}

/// The HRESULT inside a wasapi error, if it came from Windows.
pub fn hresult_of(e: &wasapi::WasapiError) -> Option<HRESULT> {
    match e {
        wasapi::WasapiError::Windows(w) => Some(w.code()),
        wasapi::WasapiError::RegisterNotifications(w) => Some(w.code()),
        _ => None,
    }
}

pub fn describe(e: &wasapi::WasapiError) -> String {
    match hresult_of(e) {
        Some(h) => format!("{e} ({:#010x})", h.0 as u32),
        None => e.to_string(),
    }
}
