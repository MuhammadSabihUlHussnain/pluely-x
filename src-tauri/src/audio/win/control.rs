//! The long-lived `audio-ctl` MTA thread: owns the `IMMDeviceEnumerator` + notification
//! client and answers device-list requests, so no COM ever runs on the main (STA) thread.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use tokio::sync::{mpsc as tmpsc, oneshot};

use super::com::ComGuard;
use super::{devices, notify};
use crate::audio::{DeviceEvent, DeviceLists, DeviceWatcher};

enum CtlMsg {
    List(oneshot::Sender<Result<DeviceLists, String>>),
    Subscribe(tmpsc::UnboundedSender<DeviceEvent>),
    Notify(DeviceEvent),
    Shutdown,
}

/// Handle to the `audio-ctl` thread. Managed state; one per app.
pub struct AudioControl {
    tx: Sender<CtlMsg>,
    thread: Option<JoinHandle<()>>,
}

impl AudioControl {
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel::<CtlMsg>();
        let notify_tx = tx.clone();
        let thread = std::thread::Builder::new()
            .name("audio-ctl".into())
            .spawn(move || run(rx, notify_tx))
            .map_err(|e| tracing::error!("failed to spawn audio-ctl thread: {e}"))
            .ok();
        Self { tx, thread }
    }

    /// Active inputs and outputs (runs on the audio-ctl thread).
    pub async fn list(&self) -> Result<DeviceLists, String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(CtlMsg::List(reply))
            .map_err(|_| "audio control thread is not running".to_string())?;
        rx.await
            .map_err(|_| "audio control thread stopped".to_string())?
    }
}

impl DeviceWatcher for AudioControl {
    fn subscribe(&self) -> tmpsc::UnboundedReceiver<DeviceEvent> {
        let (tx, rx) = tmpsc::unbounded_channel();
        let _ = self.tx.send(CtlMsg::Subscribe(tx));
        rx
    }
}

impl Drop for AudioControl {
    fn drop(&mut self) {
        let _ = self.tx.send(CtlMsg::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(rx: Receiver<CtlMsg>, notify_tx: Sender<CtlMsg>) {
    let _com = match ComGuard::new() {
        Ok(g) => Some(g),
        Err(e) => {
            tracing::error!("audio-ctl: {e}");
            None
        }
    };
    // Register for endpoint notifications. Failure only loses device-follow; listing works.
    let registration = devices::enumerator().ok().and_then(|en| {
        let client = notify::channel_client(notify_tx, CtlMsg::Notify);
        // SAFETY: valid enumerator + COM object created on this MTA thread.
        match unsafe { en.RegisterEndpointNotificationCallback(&client) } {
            Ok(()) => Some((en, client)),
            Err(e) => {
                tracing::warn!("audio-ctl: RegisterEndpointNotificationCallback failed: {e}");
                None
            }
        }
    });

    let mut subscribers: Vec<tmpsc::UnboundedSender<DeviceEvent>> = Vec::new();
    while let Ok(msg) = rx.recv() {
        match msg {
            CtlMsg::List(reply) => {
                let _ = reply.send(devices::list_all());
            }
            CtlMsg::Subscribe(s) => subscribers.push(s),
            CtlMsg::Notify(ev) => {
                tracing::debug!(?ev, "audio device event");
                subscribers.retain(|s| s.send(ev.clone()).is_ok());
            }
            CtlMsg::Shutdown => break,
        }
    }

    if let Some((en, client)) = registration {
        // SAFETY: same enumerator/client pair that was registered above.
        let _ = unsafe { en.UnregisterEndpointNotificationCallback(&client) };
    }
}
