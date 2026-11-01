//! Where session events go: the caller's `Channel<SessionEvent>` (swappable on reattach),
//! plus the last 50 finals for the `snapshot` a reloaded webview gets first.

use std::collections::VecDeque;
use std::sync::Mutex;

use super::types::{SessionEvent, SessionStatus};

pub const RECENT_FINALS: usize = 50;

/// Something that delivers events to the UI. `false` = the receiver is gone.
pub trait EventSink: Send + Sync + 'static {
    fn send(&self, ev: &SessionEvent) -> bool;
}

impl EventSink for tauri::ipc::Channel<SessionEvent> {
    fn send(&self, ev: &SessionEvent) -> bool {
        tauri::ipc::Channel::send(self, ev.clone()).is_ok()
    }
}

/// Test/diagnostic sink that keeps everything.
#[derive(Default)]
pub struct VecSink(pub Mutex<Vec<SessionEvent>>);

impl EventSink for std::sync::Arc<VecSink> {
    fn send(&self, ev: &SessionEvent) -> bool {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).push(ev.clone());
        true
    }
}

struct Inner {
    sink: Option<Box<dyn EventSink>>,
    recent: VecDeque<SessionEvent>,
    status: SessionStatus,
}

pub struct EventHub {
    inner: Mutex<Inner>,
}

impl EventHub {
    pub fn new(sink: Box<dyn EventSink>, status: SessionStatus) -> Self {
        Self {
            inner: Mutex::new(Inner {
                sink: Some(sink),
                recent: VecDeque::with_capacity(RECENT_FINALS),
                status,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn emit(&self, ev: SessionEvent) {
        let mut g = self.lock();
        match &ev {
            SessionEvent::Transcript { is_final: true, .. } => {
                if g.recent.len() == RECENT_FINALS {
                    g.recent.pop_front();
                }
                g.recent.push_back(ev.clone());
            }
            SessionEvent::TranscriptPatch { seg_id, echo } => {
                for r in g.recent.iter_mut() {
                    if let SessionEvent::Transcript { seg_id: s, echo: e, .. } = r {
                        if s == seg_id {
                            *e = Some(*echo);
                        }
                    }
                }
            }
            _ => {}
        }
        if let Some(sink) = g.sink.as_ref() {
            if !sink.send(&ev) {
                // Webview gone (reload); keep running until someone reattaches.
                g.sink = None;
            }
        }
    }

    pub fn set_status(&self, status: SessionStatus) {
        self.lock().status = status;
    }

    #[cfg(test)]
    pub fn snapshot(&self) -> SessionEvent {
        let g = self.lock();
        SessionEvent::Snapshot {
            status: g.status.clone(),
            recent_finals: g.recent.iter().cloned().collect(),
        }
    }

    /// Swap in a new sink, sending it the snapshot first (atomically w.r.t. `emit`).
    pub fn reattach(&self, sink: Box<dyn EventSink>) {
        let mut g = self.lock();
        let snap = SessionEvent::Snapshot {
            status: g.status.clone(),
            recent_finals: g.recent.iter().cloned().collect(),
        };
        if sink.send(&snap) {
            g.sink = Some(sink);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Source;
    use std::sync::Arc;

    fn final_(seg: &str, n: i64) -> SessionEvent {
        SessionEvent::Transcript {
            source: Source::Me,
            seg_id: seg.into(),
            conn: 1,
            is_final: true,
            speech_final: false,
            from_finalize: false,
            text: format!("t{n}"),
            confidence: 1.0,
            t0_ms: n,
            t1_ms: n + 1,
            speaker: None,
            echo: Some(false),
        }
    }

    #[test]
    fn keeps_last_50_finals_and_applies_patches() {
        let sink = Arc::new(VecSink::default());
        let hub = EventHub::new(Box::new(sink.clone()), SessionStatus { active: true, session_id: Some("s".into()), started_at_ms: Some(1) });
        for i in 0..60 {
            hub.emit(final_(&format!("me-1-{i}"), i));
        }
        hub.emit(SessionEvent::TranscriptPatch { seg_id: "me-1-59".into(), echo: true });
        let SessionEvent::Snapshot { recent_finals, status } = hub.snapshot() else { panic!() };
        assert!(status.active);
        assert_eq!(recent_finals.len(), 50);
        assert!(matches!(&recent_finals[0], SessionEvent::Transcript { seg_id, .. } if seg_id == "me-1-10"));
        assert!(matches!(recent_finals.last().unwrap(), SessionEvent::Transcript { echo: Some(true), .. }));
        assert_eq!(sink.0.lock().unwrap().len(), 61);
    }

    #[test]
    fn reattach_sends_snapshot_first() {
        let a = Arc::new(VecSink::default());
        let hub = EventHub::new(Box::new(a.clone()), SessionStatus::default());
        hub.emit(final_("me-1-0", 0));
        let b = Arc::new(VecSink::default());
        hub.reattach(Box::new(b.clone()));
        hub.emit(final_("me-1-1", 1));
        let got = b.0.lock().unwrap().clone();
        assert!(matches!(&got[0], SessionEvent::Snapshot { recent_finals, .. } if recent_finals.len() == 1));
        assert_eq!(got.len(), 2);
        assert_eq!(a.0.lock().unwrap().len(), 1, "old sink no longer receives");
    }

    #[test]
    fn channel_sink_serializes_json() {
        let got = Arc::new(Mutex::new(Vec::<String>::new()));
        let g2 = got.clone();
        let ch: tauri::ipc::Channel<SessionEvent> = tauri::ipc::Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(s) = body {
                g2.lock().unwrap().push(s);
            }
            Ok(())
        });
        let hub = EventHub::new(Box::new(ch), SessionStatus::default());
        hub.emit(SessionEvent::state(None, super::super::types::StreamState::Live));
        let v: serde_json::Value = serde_json::from_str(&got.lock().unwrap()[0]).unwrap();
        assert_eq!(v, serde_json::json!({ "type": "state", "source": null, "state": "live" }));
    }
}
