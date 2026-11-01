//! Speaker-turn detection (pure).
//!
//! Collects `is_final` segments per source and emits exactly one turn when Deepgram says
//! the speaker finished: on `speech_final`, or on `UtteranceEnd` if finals are still
//! pending (so speech_final followed by UtteranceEnd yields one turn, not two). `stop`
//! flushes whatever is left when the source or session stops.

use serde::{Deserialize, Serialize};

use crate::audio::Source;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnReason {
    SpeechFinal,
    UtteranceEnd,
    Stop,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub source: Source,
    pub turn_id: String,
    pub text: String,
    pub t0_ms: i64,
    pub t1_ms: i64,
    pub reason: TurnReason,
    pub question: bool,
}

#[derive(Debug)]
pub struct TurnTracker {
    source: Source,
    pending: Vec<(String, i64, i64)>,
    next_id: u32,
}

/// The cheap question hint: the turn ends with `?`.
pub fn looks_like_question(text: &str) -> bool {
    text.trim_end_matches(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ')')
        .ends_with('?')
}

impl TurnTracker {
    pub fn new(source: Source) -> Self {
        Self {
            source,
            pending: Vec::new(),
            next_id: 1,
        }
    }

    /// A final segment. Empty text still counts as a speech_final marker.
    pub fn on_final(&mut self, text: &str, t0_ms: i64, t1_ms: i64, speech_final: bool) -> Option<Turn> {
        let text = text.trim();
        if !text.is_empty() {
            self.pending.push((text.to_string(), t0_ms, t1_ms));
        }
        if speech_final {
            self.emit(TurnReason::SpeechFinal)
        } else {
            None
        }
    }

    pub fn on_utterance_end(&mut self) -> Option<Turn> {
        self.emit(TurnReason::UtteranceEnd)
    }

    pub fn flush(&mut self) -> Option<Turn> {
        self.emit(TurnReason::Stop)
    }

    fn emit(&mut self, reason: TurnReason) -> Option<Turn> {
        if self.pending.is_empty() {
            return None;
        }
        let parts = std::mem::take(&mut self.pending);
        let t0_ms = parts.iter().map(|p| p.1).min().unwrap_or(0);
        let t1_ms = parts.iter().map(|p| p.2).max().unwrap_or(t0_ms);
        let text = parts
            .into_iter()
            .map(|p| p.0)
            .collect::<Vec<_>>()
            .join(" ");
        let turn_id = format!("{}-t{}", self.source.as_str(), self.next_id);
        self.next_id += 1;
        Some(Turn {
            source: self.source,
            turn_id,
            question: looks_like_question(&text),
            text,
            t0_ms,
            t1_ms,
            reason,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    enum Ev {
        Final(&'static str, i64, i64, bool),
        UttEnd,
        Stop,
    }

    fn run(events: &[Ev]) -> Vec<Turn> {
        let mut t = TurnTracker::new(Source::Them);
        let mut out = Vec::new();
        for e in events {
            let r = match *e {
                Ev::Final(s, a, b, sf) => t.on_final(s, a, b, sf),
                Ev::UttEnd => t.on_utterance_end(),
                Ev::Stop => t.flush(),
            };
            out.extend(r);
        }
        out
    }

    #[test]
    fn speech_final_then_utterance_end_is_one_turn() {
        let turns = run(&[
            Ev::Final("So what would you do", 3500, 5100, false),
            Ev::Final("differently next time?", 5100, 6040, true),
            Ev::UttEnd,
        ]);
        assert_eq!(turns.len(), 1);
        let t = &turns[0];
        assert_eq!(t.text, "So what would you do differently next time?");
        assert_eq!((t.t0_ms, t.t1_ms), (3500, 6040));
        assert_eq!(t.reason, TurnReason::SpeechFinal);
        assert!(t.question);
        assert_eq!(t.turn_id, "them-t1");
    }

    #[test]
    fn utterance_end_without_speech_final() {
        let turns = run(&[Ev::Final("I worked at Acme.", 0, 900, false), Ev::UttEnd, Ev::UttEnd]);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].reason, TurnReason::UtteranceEnd);
        assert!(!turns[0].question);
    }

    #[test]
    fn empty_speech_final_flushes_pending() {
        let turns = run(&[
            Ev::Final("Tell me about yourself", 0, 1200, false),
            Ev::Final("", 1200, 1500, true),
        ]);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].text, "Tell me about yourself");
    }

    #[test]
    fn nothing_pending_means_no_turn() {
        assert!(run(&[Ev::UttEnd, Ev::Final("", 0, 10, true), Ev::Stop]).is_empty());
    }

    #[test]
    fn stop_flushes_and_ids_increase() {
        let turns = run(&[
            Ev::Final("one?", 0, 500, true),
            Ev::Final("two", 1000, 1500, false),
            Ev::Stop,
        ]);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].turn_id, "them-t1");
        assert_eq!(turns[1].turn_id, "them-t2");
        assert_eq!(turns[1].reason, TurnReason::Stop);
    }

    #[test]
    fn question_hint() {
        assert!(looks_like_question("Why? "));
        assert!(looks_like_question("He asked \"why?\""));
        assert!(!looks_like_question("Why not."));
        assert!(!looks_like_question(""));
    }

    #[test]
    fn reason_serializes_camel_case() {
        assert_eq!(serde_json::to_string(&TurnReason::SpeechFinal).unwrap(), "\"speechFinal\"");
        assert_eq!(serde_json::to_string(&TurnReason::UtteranceEnd).unwrap(), "\"utteranceEnd\"");
        assert_eq!(serde_json::to_string(&TurnReason::Stop).unwrap(), "\"stop\"");
    }
}
