//! Wire types for docs/CONTRACT.md §3 (camelCase JSON).

use serde::{Deserialize, Deserializer, Serialize};

use crate::audio::level::Level;
use crate::audio::{AecMode, AecState, Source};
use crate::stt::turns::TurnReason;

fn default_language() -> String {
    "en".into()
}
fn default_endpointing() -> u32 {
    300
}
fn default_utterance_end() -> u32 {
    1000
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceOptions {
    pub enabled: bool,
    /// null/absent = default (comms mic for `me`, console output loopback for `them`).
    #[serde(default)]
    pub device_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SttOptions {
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub keyterms: Vec<String>,
    #[serde(default = "default_endpointing")]
    pub endpointing_ms: u32,
    #[serde(default = "default_utterance_end")]
    pub utterance_end_ms: u32,
    #[serde(default)]
    pub diarize_them: bool,
}

impl Default for SttOptions {
    fn default() -> Self {
        Self {
            language: default_language(),
            keyterms: Vec::new(),
            endpointing_ms: default_endpointing(),
            utterance_end_ms: default_utterance_end(),
            diarize_them: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOptions {
    pub me: SourceOptions,
    pub them: SourceOptions,
    #[serde(default)]
    pub stt: SttOptions,
    #[serde(default)]
    pub aec: AecMode,
    #[serde(default = "default_true")]
    pub echo_dedup: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub device_id: String,
    pub name: String,
    pub format: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub started_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub me: Option<DeviceInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub them: Option<DeviceInfo>,
    pub aec: AecState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_id: String,
    pub duration_ms: i64,
    pub reconnects: u64,
    pub dropped_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatus {
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
}

/// `session://status` broadcast payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusBroadcast {
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamState {
    Connecting,
    Live,
    Reconnecting,
    Idle,
    Stopping,
    Stopped,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceReason {
    Opened,
    DefaultChanged,
    Lost,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionEvent {
    State {
        source: Option<Source>,
        state: StreamState,
        #[serde(skip_serializing_if = "Option::is_none")]
        attempt: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_in_ms: Option<u64>,
    },
    Device {
        source: Source,
        reason: DeviceReason,
        device_id: String,
        name: String,
        format: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        aec: Option<AecState>,
    },
    Transcript {
        source: Source,
        seg_id: String,
        conn: u32,
        is_final: bool,
        speech_final: bool,
        from_finalize: bool,
        text: String,
        confidence: f64,
        t0_ms: i64,
        t1_ms: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        speaker: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        echo: Option<bool>,
    },
    TranscriptPatch {
        seg_id: String,
        echo: bool,
    },
    SpeechStarted {
        source: Source,
        t_ms: i64,
    },
    UtteranceEnd {
        source: Source,
        last_word_end_ms: i64,
    },
    Turn {
        source: Source,
        turn_id: String,
        text: String,
        t0_ms: i64,
        t1_ms: i64,
        reason: TurnReason,
        question: bool,
    },
    Levels {
        t_ms: i64,
        me: Option<Level>,
        them: Option<Level>,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<Source>,
        code: String,
        message: String,
        fatal: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_in_ms: Option<u64>,
    },
    Stats {
        source: Source,
        sent_ms: u64,
        dropped_ms: u64,
        reconnects: u64,
        discontinuities: u64,
    },
    Snapshot {
        status: SessionStatus,
        recent_finals: Vec<SessionEvent>,
    },
}

impl SessionEvent {
    pub fn state(source: Option<Source>, state: StreamState) -> Self {
        SessionEvent::State {
            source,
            state,
            attempt: None,
            retry_in_ms: None,
        }
    }

    pub fn error(source: Option<Source>, code: impl Into<String>, message: impl Into<String>, fatal: bool, retry_in_ms: Option<u64>) -> Self {
        SessionEvent::Error {
            source,
            code: code.into(),
            message: message.into(),
            fatal,
            retry_in_ms,
        }
    }
}

/// `session_set_source` args. `deviceId` distinguishes absent (unchanged) from `null`
/// (follow the default).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSourceArgs {
    pub source: Source,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub muted: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    pub device_id: Option<Option<String>>,
}

fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_options_from_contract_json() {
        let o: SessionOptions = serde_json::from_value(json!({
            "me": { "enabled": true, "deviceId": null },
            "them": { "enabled": true },
            "stt": { "language": "en", "keyterms": ["Rust"], "endpointingMs": 300, "utteranceEndMs": 1000, "diarizeThem": true },
            "aec": "auto",
            "echoDedup": true
        }))
        .unwrap();
        assert!(o.me.enabled && o.me.device_id.is_none());
        assert_eq!(o.stt.keyterms, vec!["Rust"]);
        assert!(o.stt.diarize_them);
        assert_eq!(o.aec, AecMode::Auto);
    }

    #[test]
    fn events_serialize_like_the_contract() {
        let v = serde_json::to_value(SessionEvent::state(None, StreamState::Live)).unwrap();
        assert_eq!(v, json!({ "type": "state", "source": null, "state": "live" }));

        let v = serde_json::to_value(SessionEvent::State {
            source: Some(Source::Them),
            state: StreamState::Reconnecting,
            attempt: Some(2),
            retry_in_ms: Some(480),
        })
        .unwrap();
        assert_eq!(
            v,
            json!({ "type": "state", "source": "them", "state": "reconnecting", "attempt": 2, "retryInMs": 480 })
        );

        let v = serde_json::to_value(SessionEvent::Transcript {
            source: Source::Me,
            seg_id: "me-1-0".into(),
            conn: 1,
            is_final: true,
            speech_final: false,
            from_finalize: false,
            text: "hi".into(),
            confidence: 0.5,
            t0_ms: 0,
            t1_ms: 100,
            speaker: None,
            echo: Some(true),
        })
        .unwrap();
        assert_eq!(
            v,
            json!({ "type": "transcript", "source": "me", "segId": "me-1-0", "conn": 1, "isFinal": true,
                    "speechFinal": false, "fromFinalize": false, "text": "hi", "confidence": 0.5,
                    "t0Ms": 0, "t1Ms": 100, "echo": true })
        );

        let v = serde_json::to_value(SessionEvent::TranscriptPatch { seg_id: "me-1-0".into(), echo: true }).unwrap();
        assert_eq!(v, json!({ "type": "transcriptPatch", "segId": "me-1-0", "echo": true }));

        let v = serde_json::to_value(SessionEvent::UtteranceEnd { source: Source::Them, last_word_end_ms: 5 }).unwrap();
        assert_eq!(v, json!({ "type": "utteranceEnd", "source": "them", "lastWordEndMs": 5 }));

        let v = serde_json::to_value(SessionEvent::SpeechStarted { source: Source::Them, t_ms: 5 }).unwrap();
        assert_eq!(v, json!({ "type": "speechStarted", "source": "them", "tMs": 5 }));

        let v = serde_json::to_value(SessionEvent::Levels { t_ms: 1, me: Some(Level::FLOOR), them: None }).unwrap();
        assert_eq!(v, json!({ "type": "levels", "tMs": 1, "me": { "rmsDb": -90.0, "peakDb": -90.0 }, "them": null }));

        let v = serde_json::to_value(SessionEvent::Device {
            source: Source::Them,
            reason: DeviceReason::DefaultChanged,
            device_id: "x".into(),
            name: "Speakers".into(),
            format: "48 kHz 2 ch → 16 kHz mono".into(),
            aec: None,
        })
        .unwrap();
        assert_eq!(v["reason"], "defaultChanged");
        assert!(v.get("aec").is_none());

        let v = serde_json::to_value(SessionEvent::error(Some(Source::Me), "dg_auth", "bad key", true, None)).unwrap();
        assert_eq!(v, json!({ "type": "error", "source": "me", "code": "dg_auth", "message": "bad key", "fatal": true }));

        let v = serde_json::to_value(SessionEvent::Turn {
            source: Source::Them,
            turn_id: "them-t1".into(),
            text: "why?".into(),
            t0_ms: 1,
            t1_ms: 2,
            reason: TurnReason::UtteranceEnd,
            question: true,
        })
        .unwrap();
        assert_eq!(v["reason"], "utteranceEnd");
        assert_eq!(v["turnId"], "them-t1");

        let v = serde_json::to_value(SessionEvent::Stats { source: Source::Me, sent_ms: 1, dropped_ms: 2, reconnects: 3, discontinuities: 4 }).unwrap();
        assert_eq!(v, json!({ "type": "stats", "source": "me", "sentMs": 1, "droppedMs": 2, "reconnects": 3, "discontinuities": 4 }));

        let v = serde_json::to_value(SessionEvent::Snapshot {
            status: SessionStatus { active: true, session_id: Some("s".into()), started_at_ms: Some(9) },
            recent_finals: vec![],
        })
        .unwrap();
        assert_eq!(v, json!({ "type": "snapshot", "status": { "active": true, "sessionId": "s", "startedAtMs": 9 }, "recentFinals": [] }));
    }

    #[test]
    fn info_and_summary() {
        let v = serde_json::to_value(SessionInfo {
            session_id: "s".into(),
            started_at_ms: 1,
            me: None,
            them: Some(DeviceInfo { device_id: "d".into(), name: "n".into(), format: "f".into() }),
            aec: AecState::Unsupported,
        })
        .unwrap();
        assert_eq!(
            v,
            json!({ "sessionId": "s", "startedAtMs": 1, "them": { "deviceId": "d", "name": "n", "format": "f" }, "aec": "unsupported" })
        );
        let v = serde_json::to_value(SessionSummary { session_id: "s".into(), duration_ms: 5, reconnects: 1, dropped_ms: 50 }).unwrap();
        assert_eq!(v, json!({ "sessionId": "s", "durationMs": 5, "reconnects": 1, "droppedMs": 50 }));
    }

    #[test]
    fn set_source_device_id_absent_vs_null() {
        let a: SetSourceArgs = serde_json::from_value(json!({ "source": "me", "muted": true })).unwrap();
        assert_eq!(a.device_id, None);
        assert_eq!(a.muted, Some(true));
        let b: SetSourceArgs = serde_json::from_value(json!({ "source": "me", "deviceId": null })).unwrap();
        assert_eq!(b.device_id, Some(None));
        let c: SetSourceArgs = serde_json::from_value(json!({ "source": "them", "deviceId": "abc", "enabled": false })).unwrap();
        assert_eq!(c.device_id, Some(Some("abc".into())));
        assert_eq!(c.enabled, Some(false));
    }
}
