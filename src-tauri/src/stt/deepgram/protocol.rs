//! Deepgram streaming wire format (pure serde).
//!
//! Server → client: `Results`, `UtteranceEnd`, `SpeechStarted`, `Metadata`; anything else
//! parses as `Unknown` so a new message type never breaks the socket.
//! Client → server control messages are TEXT frames: KeepAlive, Finalize, CloseStream.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMsg {
    Results(ResultsMsg),
    UtteranceEnd(UtteranceEndMsg),
    SpeechStarted(SpeechStartedMsg),
    Metadata(MetadataMsg),
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ResultsMsg {
    #[serde(default)]
    pub channel_index: Vec<u32>,
    /// Seconds of connection audio at which this segment starts. Interims for the same
    /// segment share `start` until the final.
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub is_final: bool,
    #[serde(default)]
    pub speech_final: bool,
    #[serde(default)]
    pub from_finalize: bool,
    #[serde(default)]
    pub channel: ResultsChannel,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct ResultsChannel {
    #[serde(default)]
    pub alternatives: Vec<Alternative>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct Alternative {
    #[serde(default)]
    pub transcript: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Word {
    #[serde(default)]
    pub word: String,
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub end: f64,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub punctuated_word: Option<String>,
    #[serde(default)]
    pub speaker: Option<u32>,
}

impl ResultsMsg {
    fn best(&self) -> Option<&Alternative> {
        self.channel.alternatives.first()
    }

    pub fn transcript(&self) -> &str {
        self.best().map(|a| a.transcript.trim()).unwrap_or("")
    }

    pub fn confidence(&self) -> f64 {
        self.best().map(|a| a.confidence).unwrap_or(0.0)
    }

    /// Majority speaker among the words (diarization), ties → lowest id.
    pub fn speaker(&self) -> Option<u32> {
        let words = &self.best()?.words;
        let mut counts: Vec<(u32, usize)> = Vec::new();
        for s in words.iter().filter_map(|w| w.speaker) {
            match counts.iter_mut().find(|(id, _)| *id == s) {
                Some((_, n)) => *n += 1,
                None => counts.push((s, 1)),
            }
        }
        counts
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(id, _)| id)
    }

    /// `"{source}-{conn}-{round(start*1000)}"`, stable across interims of one segment.
    pub fn seg_id(&self, source: &str, conn: u32) -> String {
        format!("{source}-{conn}-{}", (self.start * 1000.0).round() as i64)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UtteranceEndMsg {
    #[serde(default)]
    pub channel: Vec<u32>,
    #[serde(default)]
    pub last_word_end: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SpeechStartedMsg {
    #[serde(default)]
    pub channel: Vec<u32>,
    #[serde(default)]
    pub timestamp: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MetadataMsg {
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub channels: Option<u32>,
}

pub fn parse(text: &str) -> Result<ServerMsg, serde_json::Error> {
    serde_json::from_str(text)
}

pub const KEEP_ALIVE: &str = r#"{"type":"KeepAlive"}"#;
pub const FINALIZE: &str = r#"{"type":"Finalize"}"#;
pub const CLOSE_STREAM: &str = r#"{"type":"CloseStream"}"#;

#[cfg(test)]
mod tests {
    use super::*;

    const INTERIM: &str = include_str!("fixtures/interim.json");
    const FINAL: &str = include_str!("fixtures/final.json");
    const SPEECH_FINAL: &str = include_str!("fixtures/speech_final.json");
    const UTTERANCE_END: &str = include_str!("fixtures/utterance_end.json");
    const SPEECH_STARTED: &str = include_str!("fixtures/speech_started.json");
    const METADATA: &str = include_str!("fixtures/metadata.json");
    const UNKNOWN: &str = include_str!("fixtures/unknown.json");

    fn results(s: &str) -> ResultsMsg {
        match parse(s).unwrap() {
            ServerMsg::Results(r) => r,
            other => panic!("expected Results, got {other:?}"),
        }
    }

    #[test]
    fn interim() {
        let r = results(INTERIM);
        assert!(!r.is_final && !r.speech_final && !r.from_finalize);
        assert_eq!(r.start, 3.5);
        assert_eq!(r.duration, 1.02);
        assert_eq!(r.transcript(), "so what would you");
        assert!((r.confidence() - 0.8712).abs() < 1e-9);
        assert_eq!(r.speaker(), None);
        assert_eq!(r.seg_id("them", 2), "them-2-3500");
        assert_eq!(r.channel.alternatives[0].words.len(), 4);
        assert_eq!(r.channel.alternatives[0].words[0].punctuated_word.as_deref(), Some("So"));
    }

    #[test]
    fn final_shares_seg_id_with_its_interims() {
        let r = results(FINAL);
        assert!(r.is_final && !r.speech_final);
        assert_eq!(r.transcript(), "So what would you do");
        assert_eq!(r.seg_id("them", 2), results(INTERIM).seg_id("them", 2));
        assert_eq!(r.speaker(), Some(1), "majority speaker");
    }

    #[test]
    fn speech_final() {
        let r = results(SPEECH_FINAL);
        assert!(r.is_final && r.speech_final && r.from_finalize);
        assert_eq!(r.transcript(), "differently next time?");
        assert_eq!(r.seg_id("me", 1), "me-1-5100");
    }

    #[test]
    fn utterance_end() {
        assert_eq!(
            parse(UTTERANCE_END).unwrap(),
            ServerMsg::UtteranceEnd(UtteranceEndMsg {
                channel: vec![0, 1],
                last_word_end: 6.02
            })
        );
    }

    #[test]
    fn speech_started() {
        assert_eq!(
            parse(SPEECH_STARTED).unwrap(),
            ServerMsg::SpeechStarted(SpeechStartedMsg {
                channel: vec![0],
                timestamp: 3.52
            })
        );
    }

    #[test]
    fn metadata() {
        match parse(METADATA).unwrap() {
            ServerMsg::Metadata(m) => {
                assert_eq!(m.request_id.as_deref(), Some("7f9c1a52-1e4b-4b8e-9d2f-2b3c4d5e6f70"));
                assert_eq!(m.duration, Some(12.5));
                assert_eq!(m.channels, Some(1));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_types_dont_fail() {
        assert_eq!(parse(UNKNOWN).unwrap(), ServerMsg::Unknown);
        assert_eq!(parse(r#"{"type":"Error","description":"x"}"#).unwrap(), ServerMsg::Unknown);
        assert!(parse("not json").is_err());
        assert!(parse(r#"{"no_type":1}"#).is_err());
    }

    #[test]
    fn empty_results_are_tolerated() {
        let r = results(r#"{"type":"Results","is_final":true,"channel":{"alternatives":[]}}"#);
        assert_eq!(r.transcript(), "");
        assert_eq!(r.confidence(), 0.0);
        let r = results(r#"{"type":"Results"}"#);
        assert_eq!(r.transcript(), "");
    }

    #[test]
    fn control_messages() {
        for (s, t) in [(KEEP_ALIVE, "KeepAlive"), (FINALIZE, "Finalize"), (CLOSE_STREAM, "CloseStream")] {
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            assert_eq!(v, serde_json::json!({ "type": t }));
        }
    }
}
