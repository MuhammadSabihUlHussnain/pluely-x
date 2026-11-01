//! COM apartment guard. Every thread that touches WASAPI (`audio-ctl`, `cap-me`, `cap-them`)
//! joins the MTA first and leaves it when done. COM objects never cross threads.

use std::marker::PhantomData;

pub struct ComGuard {
    // COM init is per-thread: keep the guard on the thread that created it.
    _not_send: PhantomData<*const ()>,
}

impl ComGuard {
    /// `CoInitializeEx(MTA)`. S_OK and S_FALSE (already initialised) both succeed;
    /// RPC_E_CHANGED_MODE (thread is STA) fails and must not be balanced by CoUninitialize.
    pub fn new() -> Result<Self, String> {
        let hr = wasapi::initialize_mta();
        if hr.is_ok() {
            Ok(Self {
                _not_send: PhantomData,
            })
        } else {
            Err(format!("CoInitializeEx(MTA) failed: {:#010x}", hr.0 as u32))
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        wasapi::deinitialize();
    }
}
