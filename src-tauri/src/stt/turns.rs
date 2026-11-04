//! Speaker-turn detection (pure).
//!
//! Collects `is_final` segments per source and emits exactly one turn when Deepgram says
//! the speaker finished: on `speech_final`, on a final flushed by `Finalize` (sent when
//! the source's audio went idle), or on `UtteranceEnd` if finals are still pending (so
//! speech_final followed by UtteranceEnd yields one turn, not two). `stop` flushes
//! whatever is left when the source or session stops.
//!
//! Turn ids are unique across sessions (`them-t1-<session tag>`): the frontend keeps the
//! ids it has answered, and plain `them-t1` repeating in the next session once made it
//! ignore that session's first questions.

use serde::{Deserialize, Serialize};

use crate::audio::Source;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnReason {
    SpeechFinal,
    UtteranceEnd,
    /// Deepgram flushed the pending audio after an idle `Finalize`.
    Finalize,
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
    /// Appended to every turn id (empty: plain `them-t1`).
    tag: String,
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
            tag: String::new(),
        }
    }

    /// A tracker whose ids carry the session (first 8 characters of its id), so they never
    /// repeat across sessions.
    pub fn for_session(source: Source, session_id: &str) -> Self {
        let tag: String = session_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
        Self { tag, ..Self::new(source) }
    }

    /// A final segment. Empty text still counts as a speech_final / finalize marker.
    pub fn on_final(
        &mut self,
        text: &str,
        t0_ms: i64,
        t1_ms: i64,
        speech_final: bool,
        from_finalize: bool,
    ) -> Option<Turn> {
        let text = text.trim();
        if !text.is_empty() {
            self.pending.push((text.to_string(), t0_ms, t1_ms));
        }
        if speech_final {
            self.emit(TurnReason::SpeechFinal)
        } else if from_finalize {
            self.emit(TurnReason::Finalize)
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
        let turn_id = if self.tag.is_empty() {
            format!("{}-t{}", self.source.as_str(), self.next_id)
        } else {
            format!("{}-t{}-{}", self.source.as_str(), self.next_id, self.tag)
        };
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
        Finalized(&'static str, i64, i64),
        UttEnd,
        Stop,
    }

    fn run(events: &[Ev]) -> Vec<Turn> {
        let mut t = TurnTracker::new(Source::Them);
        let mut out = Vec::new();
        for e in events {
            let r = match *e {
                Ev::Final(s, a, b, sf) => t.on_final(s, a, b, sf, false),
                Ev::Finalized(s, a, b) => t.on_final(s, a, b, false, true),
                Ev::UttEnd => t.on_utterance_end(),
                Ev::Stop => t.flush(),
            };
            out.extend(r);
        }
        out
    }

    #[test]
    fn session_ids_never_repeat_across_sessions() {
        let mut a = TurnTracker::for_session(Source::Them, "3f9a2b1c-77e0-4c4e-9d2b-0a1b2c3d4e5f");
        let mut b = TurnTracker::for_session(Source::Them, "8c0d1e2f-1111-2222-3333-444455556666");
        let ta = a.on_final("Why banking?", 0, 900, true, false).unwrap();
        let tb = b.on_final("Why banking?", 0, 900, true, false).unwrap();
        assert_eq!(ta.turn_id, "them-t1-3f9a2b1c");
        assert_eq!(tb.turn_id, "them-t1-8c0d1e2f");
        assert_eq!(a.on_final("And why us?", 1000, 1900, true, false).unwrap().turn_id, "them-t2-3f9a2b1c");
        let mut me = TurnTracker::for_session(Source::Me, "3f9a2b1c-77e0");
        assert_eq!(me.on_final("Sure.", 0, 400, true, false).unwrap().turn_id, "me-t1-3f9a2b1c");
    }

    #[test]
    fn idle_finalize_ends_the_turn_once() {
        // The source went idle mid-question: Deepgram flushes on Finalize, and a late
        // UtteranceEnd must not create a second turn.
        let turns = run(&[
            Ev::Final("Can you tell me about a project", 13400, 15200, false),
            Ev::Finalized("you're really proud of?", 15200, 18360),
            Ev::UttEnd,
        ]);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].reason, TurnReason::Finalize);
        assert_eq!(turns[0].text, "Can you tell me about a project you're really proud of?");
        assert!(turns[0].question);
        assert!(run(&[Ev::Finalized("", 0, 10)]).is_empty(), "empty flush with nothing pending");
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
