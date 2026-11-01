//! Endpoint lookup and listing. Everything here must run on an MTA thread (see `com.rs`).

use wasapi::{DeviceCollection, DeviceState, Direction, Role};
use windows::core::HSTRING;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

use super::{describe, hr};
use crate::audio::{CaptureError, Device, DeviceLists, DeviceSelector, Source};

pub fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    // SAFETY: plain COM activation on an MTA thread.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

/// The endpoint direction a source captures from (loopback captures a render endpoint).
pub fn direction_for(source: Source) -> Direction {
    match source {
        Source::Me => Direction::Capture,
        Source::Them => Direction::Render,
    }
}

/// The default role a source follows: Communications mic, Console output.
pub fn role_for(source: Source) -> Role {
    match source {
        Source::Me => Role::Communications,
        Source::Them => Role::Console,
    }
}

/// An active endpoint by id, if it exists and has the right direction.
pub fn device_by_id(
    en: &IMMDeviceEnumerator,
    id: &str,
    direction: Direction,
) -> Result<Option<wasapi::Device>, CaptureError> {
    let hid = HSTRING::from(id);
    // SAFETY: valid enumerator and a NUL-terminated wide string that outlives the call.
    let imm = match unsafe { en.GetDevice(&hid) } {
        Ok(d) => d,
        Err(e) if e.code() == hr::E_NOTFOUND => return Ok(None),
        Err(e) => {
            return Err(CaptureError::InitFailed(format!("GetDevice failed: {e}")));
        }
    };
    let dev = match wasapi::Device::from_immdevice(imm) {
        Ok(d) => d,
        Err(_) => return Ok(None),
    };
    if dev.get_direction() != direction {
        return Ok(None);
    }
    match dev.get_state() {
        Ok(DeviceState::Active) => Ok(Some(dev)),
        _ => Ok(None),
    }
}

/// Resolve what a source should open. A chosen device that's missing falls back to the
/// default (`fallback = true`).
pub fn resolve(
    source: Source,
    selector: &DeviceSelector,
) -> Result<(wasapi::Device, bool), CaptureError> {
    let direction = direction_for(source);
    let mut fallback = false;
    if let DeviceSelector::Id(id) = selector {
        let en = enumerator().map_err(|e| CaptureError::InitFailed(format!("enumerator: {e}")))?;
        if let Some(dev) = device_by_id(&en, id, direction)? {
            return Ok((dev, false));
        }
        tracing::warn!(source = %source, "chosen audio device missing; falling back to default");
        fallback = true;
    }
    match wasapi::get_default_device_for_role(&direction, &role_for(source)) {
        Ok(dev) => Ok((dev, fallback)),
        Err(e) => {
            let not_found = matches!(super::hresult_of(&e), Some(h) if h == hr::E_NOTFOUND);
            if not_found {
                Err(CaptureError::DeviceNotFound(format!(
                    "no default {} device",
                    match source {
                        Source::Me => "capture",
                        Source::Them => "render",
                    }
                )))
            } else {
                Err(CaptureError::InitFailed(describe(&e)))
            }
        }
    }
}

fn default_id(direction: Direction, role: Role) -> Option<String> {
    wasapi::get_default_device_for_role(&direction, &role)
        .ok()
        .and_then(|d| d.get_id().ok())
}

fn list_direction(direction: Direction) -> Result<Vec<Device>, String> {
    let default = default_id(direction, Role::Console);
    let comms = default_id(direction, Role::Communications);
    let collection = DeviceCollection::new(&direction).map_err(|e| describe(&e))?;
    let count = collection.get_nbr_devices().map_err(|e| describe(&e))?;
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let Ok(dev) = collection.get_device_at_index(i) else {
            continue;
        };
        let Ok(id) = dev.get_id() else {
            continue;
        };
        let name = dev.get_friendlyname().unwrap_or_else(|_| match direction {
            Direction::Capture => format!("Microphone {}", i + 1),
            Direction::Render => format!("Output {}", i + 1),
        });
        out.push(Device {
            is_default: default.as_deref() == Some(id.as_str()),
            is_default_comms: comms.as_deref() == Some(id.as_str()),
            id,
            name,
        });
    }
    Ok(out)
}

/// Active inputs and outputs with default flags.
pub fn list_all() -> Result<DeviceLists, String> {
    Ok(DeviceLists {
        inputs: list_direction(Direction::Capture)?,
        outputs: list_direction(Direction::Render)?,
    })
}
