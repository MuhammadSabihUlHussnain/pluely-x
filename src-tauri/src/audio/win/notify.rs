//! `IMMNotificationClient` (wasapi 0.19 has none). Windows calls these on its own threads;
//! the callbacks only convert arguments and `send()` into a channel. They never block and
//! never call back into COM.

use std::sync::mpsc::Sender;

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, ERole,
    IMMNotificationClient, IMMNotificationClient_Impl, DEVICE_STATE, DEVICE_STATE_ACTIVE,
};
use windows_core::{implement, PCWSTR};

use crate::audio::{DeviceEvent, DeviceRole, Flow};

type Forward = Box<dyn Fn(DeviceEvent) + Send + Sync + 'static>;

/// Forwards endpoint notifications as [`DeviceEvent`]s.
#[implement(IMMNotificationClient)]
pub struct NotificationClient {
    forward: Forward,
}

impl NotificationClient {
    pub fn new(forward: impl Fn(DeviceEvent) + Send + Sync + 'static) -> Self {
        Self {
            forward: Box::new(forward),
        }
    }
}

/// Convenience: a client that sends into a std mpsc channel.
pub fn channel_client<T: Send + 'static>(
    tx: Sender<T>,
    wrap: fn(DeviceEvent) -> T,
) -> IMMNotificationClient {
    // std Sender is Sync (Rust >= 1.72), so the closure is Send + Sync.
    NotificationClient::new(move |ev| {
        let _ = tx.send(wrap(ev));
    })
    .into()
}

fn id_of(p: &PCWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: Windows passes a valid NUL-terminated string for the duration of the call.
    unsafe { p.to_string().ok() }
}

fn flow_of(f: EDataFlow) -> Option<Flow> {
    if f == eRender {
        Some(Flow::Render)
    } else if f == eCapture {
        Some(Flow::Capture)
    } else {
        None
    }
}

fn role_of(r: ERole) -> Option<DeviceRole> {
    if r == eConsole {
        Some(DeviceRole::Console)
    } else if r == eMultimedia {
        Some(DeviceRole::Multimedia)
    } else if r == eCommunications {
        Some(DeviceRole::Communications)
    } else {
        None
    }
}

impl IMMNotificationClient_Impl for NotificationClient_Impl {
    fn OnDeviceStateChanged(
        &self,
        pwstrdeviceid: &PCWSTR,
        dwnewstate: DEVICE_STATE,
    ) -> windows_core::Result<()> {
        if let Some(id) = id_of(pwstrdeviceid) {
            (self.forward)(DeviceEvent::StateChanged {
                id,
                active: dwnewstate == DEVICE_STATE_ACTIVE,
            });
        }
        Ok(())
    }

    fn OnDeviceAdded(&self, pwstrdeviceid: &PCWSTR) -> windows_core::Result<()> {
        if let Some(id) = id_of(pwstrdeviceid) {
            (self.forward)(DeviceEvent::Added(id));
        }
        Ok(())
    }

    fn OnDeviceRemoved(&self, pwstrdeviceid: &PCWSTR) -> windows_core::Result<()> {
        if let Some(id) = id_of(pwstrdeviceid) {
            (self.forward)(DeviceEvent::Removed(id));
        }
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        pwstrdefaultdeviceid: &PCWSTR,
    ) -> windows_core::Result<()> {
        if let (Some(flow), Some(role)) = (flow_of(flow), role_of(role)) {
            (self.forward)(DeviceEvent::DefaultChanged {
                flow,
                role,
                id: id_of(pwstrdefaultdeviceid),
            });
        }
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _pwstrdeviceid: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> windows_core::Result<()> {
        Ok(())
    }
}
