//! Device lists and change notifications on macOS: a small `audio-ctl` thread that answers
//! list requests and polls Core Audio every second for default-device and device-set
//! changes, forwarding them as the same [`DeviceEvent`]s the Windows notifier produces.

use std::collections::BTreeSet;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use tokio::sync::{mpsc as tmpsc, oneshot};

use super::devices;
use crate::audio::{DeviceEvent, DeviceLists, DeviceRole, DeviceWatcher, Flow};

const POLL: Duration = Duration::from_secs(1);

enum CtlMsg {
    List(oneshot::Sender<Result<DeviceLists, String>>),
    Subscribe(tmpsc::UnboundedSender<DeviceEvent>),
    Shutdown,
}

pub struct AudioControl {
    tx: Sender<CtlMsg>,
    thread: Option<JoinHandle<()>>,
}

impl AudioControl {
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel::<CtlMsg>();
        let thread = std::thread::Builder::new()
            .name("audio-ctl".into())
            .spawn(move || run(rx))
            .map_err(|e| tracing::error!("failed to spawn audio-ctl thread: {e}"))
            .ok();
        Self { tx, thread }
    }

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

/// What the poller last saw.
#[derive(Default, PartialEq)]
struct Snapshot {
    default_in: Option<String>,
    default_out: Option<String>,
    ids: BTreeSet<String>,
}

fn snapshot() -> Snapshot {
    let (default_in, default_out) = devices::default_uids();
    let ids = devices::list_all()
        .map(|l| l.inputs.into_iter().chain(l.outputs).map(|d| d.id).collect())
        .unwrap_or_default();
    Snapshot {
        default_in,
        default_out,
        ids,
    }
}

/// The events that turn `old` into `new`. macOS has one default per direction, reported for
/// every role so whichever role a source follows sees the change.
fn diff(old: &Snapshot, new: &Snapshot) -> Vec<DeviceEvent> {
    let mut out = Vec::new();
    for id in new.ids.difference(&old.ids) {
        out.push(DeviceEvent::Added(id.clone()));
    }
    for id in old.ids.difference(&new.ids) {
        out.push(DeviceEvent::Removed(id.clone()));
    }
    let roles = [DeviceRole::Console, DeviceRole::Multimedia, DeviceRole::Communications];
    if new.default_in != old.default_in {
        for role in roles {
            out.push(DeviceEvent::DefaultChanged {
                flow: Flow::Capture,
                role,
                id: new.default_in.clone(),
            });
        }
    }
    if new.default_out != old.default_out {
        for role in roles {
            out.push(DeviceEvent::DefaultChanged {
                flow: Flow::Render,
                role,
                id: new.default_out.clone(),
            });
        }
    }
    out
}

fn run(rx: Receiver<CtlMsg>) {
    let mut subscribers: Vec<tmpsc::UnboundedSender<DeviceEvent>> = Vec::new();
    let mut last = snapshot();
    loop {
        match rx.recv_timeout(POLL) {
            Ok(CtlMsg::List(reply)) => {
                let _ = reply.send(devices::list_all());
            }
            Ok(CtlMsg::Subscribe(tx)) => subscribers.push(tx),
            Ok(CtlMsg::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {
                subscribers.retain(|s| !s.is_closed());
                if subscribers.is_empty() {
                    continue;
                }
                let now = snapshot();
                if now != last {
                    for ev in diff(&last, &now) {
                        tracing::info!(?ev, "audio device change");
                        subscribers.retain(|s| s.send(ev.clone()).is_ok());
                    }
                    last = now;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(i: Option<&str>, o: Option<&str>, ids: &[&str]) -> Snapshot {
        Snapshot {
            default_in: i.map(str::to_string),
            default_out: o.map(str::to_string),
            ids: ids.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn diff_reports_added_removed_and_default_changes() {
        let a = snap(Some("mic"), Some("spk"), &["mic", "spk"]);
        let b = snap(Some("mic"), Some("airpods"), &["mic", "spk", "airpods"]);
        let ev = diff(&a, &b);
        assert!(ev.contains(&DeviceEvent::Added("airpods".into())));
        assert!(ev.contains(&DeviceEvent::DefaultChanged {
            flow: Flow::Render,
            role: DeviceRole::Console,
            id: Some("airpods".into())
        }));
        assert!(!ev.iter().any(|e| matches!(e, DeviceEvent::DefaultChanged { flow: Flow::Capture, .. })));
        assert!(diff(&b, &b).is_empty());
        assert!(diff(&b, &a).contains(&DeviceEvent::Removed("airpods".into())));
    }
}
