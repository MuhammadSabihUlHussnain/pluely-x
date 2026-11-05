//! Core Audio device lists (ids are device UIDs, stable across reboots).

use cidre::core_audio as ca;

use crate::audio::{Device, DeviceLists};

/// Current default input and output UIDs.
pub fn default_uids() -> (Option<String>, Option<String>) {
    let input = ca::System::default_input_device()
        .ok()
        .and_then(|d| d.uid().ok())
        .map(|u| u.to_string());
    let output = ca::System::default_output_device()
        .ok()
        .and_then(|d| d.uid().ok())
        .map(|u| u.to_string());
    (input, output)
}

/// Every device with input and/or output streams. macOS has no separate
/// "communications" default, so `is_default_comms` mirrors nothing (always false); the
/// frontend's call-device check then never fires, which is right: the process tap hears
/// every output device anyway.
pub fn list_all() -> Result<DeviceLists, String> {
    let (def_in, def_out) = default_uids();
    let all = ca::System::devices().map_err(|e| format!("Core Audio device list failed: {e:?}"))?;
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for d in all.iter() {
        let Ok(uid) = d.uid().map(|u| u.to_string()) else {
            continue;
        };
        let name = d
            .name()
            .map(|n| n.to_string())
            .unwrap_or_else(|_| "Unknown device".to_string());
        let ins = d.input_stream_cfg().map(|c| c.number_buffers()).unwrap_or(0);
        let outs = d.output_stream_cfg().map(|c| c.number_buffers()).unwrap_or(0);
        if ins > 0 {
            inputs.push(Device {
                id: uid.clone(),
                name: name.clone(),
                is_default: def_in.as_deref() == Some(uid.as_str()),
                is_default_comms: false,
            });
        }
        if outs > 0 {
            outputs.push(Device {
                id: uid.clone(),
                name,
                is_default: def_out.as_deref() == Some(uid.as_str()),
                is_default_comms: false,
            });
        }
    }
    Ok(DeviceLists { inputs, outputs })
}

/// The name of the device with this UID (cpal selects devices by name).
pub fn name_for_uid(uid: &str) -> Option<String> {
    let all = ca::System::devices().ok()?;
    all.iter()
        .find(|d| d.uid().map(|u| u.to_string()).ok().as_deref() == Some(uid))
        .and_then(|d| d.name().ok())
        .map(|n| n.to_string())
}

/// The output device with this UID, if it still exists.
pub fn output_device(uid: &str) -> Option<ca::Device> {
    let all = ca::System::devices().ok()?;
    all.into_iter().find(|d| {
        d.output_stream_cfg().map(|c| c.number_buffers()).unwrap_or(0) > 0
            && d.uid().map(|u| u.to_string()).ok().as_deref() == Some(uid)
    })
}
