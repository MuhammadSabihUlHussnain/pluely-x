//! Transcript-level echo flagging (pure, always on unless `echoDedup` is off).
//!
//! Without headphones the mic hears the speakers, so "me" transcribes what "them" said.
//! For each "me" final we compare it with the "them" finals overlapping
//! [t0 − 1.5 s, t1 + 1.5 s]: score = LCS(normalised tokens) / me-token count. It's an echo
//! when the score ≥ 0.6 with ≥ 3 tokens, or the 1–2 tokens appear verbatim. The "me"
//! final is emitted immediately; if the matching "them" final arrives later, the caller
//! sends `transcriptPatch { segId, echo: true }`.

use std::collections::VecDeque;

pub const WINDOW_MS: i64 = 1_500;
pub const MIN_SCORE: f64 = 0.6;
/// How long finals are kept for matching.
pub const KEEP_MS: i64 = 30_000;
/// Token cap per comparison (keeps LCS cheap).
const MAX_TOKENS: usize = 256;

pub fn normalize(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|t| t.trim_matches('\'').to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

pub fn lcs_len(a: &[String], b: &[String]) -> usize {
    let a = &a[..a.len().min(MAX_TOKENS)];
    let b = &b[..b.len().min(MAX_TOKENS)];
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let mut prev = vec![0usize; b.len() + 1];
    let mut cur = vec![0usize; b.len() + 1];
    for x in a {
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = if x == y {
                prev[j] + 1
            } else {
                cur[j].max(prev[j + 1])
            };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn contains_run(hay: &[String], needle: &[String]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// Is `me` (normalised) an echo of `them` (normalised, concatenated)?
pub fn is_echo(me: &[String], them: &[String]) -> bool {
    match me.len() {
        0 => false,
        1 | 2 => contains_run(them, me),
        n => (lcs_len(me, them) as f64 / n as f64) >= MIN_SCORE,
    }
}

#[derive(Debug, Clone)]
struct Seg {
    seg_id: String,
    tokens: Vec<String>,
    t0: i64,
    t1: i64,
    flagged: bool,
}

#[derive(Debug, Default)]
pub struct EchoDetector {
    them: VecDeque<Seg>,
    me: VecDeque<Seg>,
}

fn overlaps(t0: i64, t1: i64, s: &Seg) -> bool {
    s.t1 >= t0 - WINDOW_MS && s.t0 <= t1 + WINDOW_MS
}

impl EchoDetector {
    pub fn new() -> Self {
        Self::default()
    }

    fn them_tokens_near(&self, t0: i64, t1: i64) -> Vec<String> {
        let mut segs: Vec<&Seg> = self.them.iter().filter(|s| overlaps(t0, t1, s)).collect();
        segs.sort_by_key(|s| s.t0);
        segs.into_iter().flat_map(|s| s.tokens.iter().cloned()).collect()
    }

    fn prune(&mut self, now: i64) {
        while self.them.front().is_some_and(|s| s.t1 < now - KEEP_MS) {
            self.them.pop_front();
        }
        while self.me.front().is_some_and(|s| s.t1 < now - KEEP_MS) {
            self.me.pop_front();
        }
    }

    /// A "me" final. Returns whether it's an echo of "them" already.
    pub fn on_me_final(&mut self, seg_id: &str, text: &str, t0: i64, t1: i64) -> bool {
        let tokens = normalize(text);
        let echo = is_echo(&tokens, &self.them_tokens_near(t0, t1));
        self.me.push_back(Seg {
            seg_id: seg_id.to_string(),
            tokens,
            t0,
            t1,
            flagged: echo,
        });
        self.prune(t1);
        echo
    }

    /// A "them" final. Returns the "me" segIds that turn out to be echoes of it.
    pub fn on_them_final(&mut self, seg_id: &str, text: &str, t0: i64, t1: i64) -> Vec<String> {
        self.them.push_back(Seg {
            seg_id: seg_id.to_string(),
            tokens: normalize(text),
            t0,
            t1,
            flagged: false,
        });
        let candidates: Vec<usize> = self
            .me
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.flagged && m.t1 >= t0 - WINDOW_MS && m.t0 <= t1 + WINDOW_MS)
            .map(|(i, _)| i)
            .collect();
        let mut patched = Vec::new();
        for i in candidates {
            let (mt0, mt1) = (self.me[i].t0, self.me[i].t1);
            let them = self.them_tokens_near(mt0, mt1);
            if is_echo(&self.me[i].tokens, &them) {
                self.me[i].flagged = true;
                patched.push(self.me[i].seg_id.clone());
            }
        }
        self.prune(t1);
        patched
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<String> {
        normalize(s)
    }

    #[test]
    fn normalization() {
        assert_eq!(toks("Hello, World! It's 5pm."), vec!["hello", "world", "it's", "5pm"]);
        assert_eq!(toks("  ...  "), Vec::<String>::new());
    }

    #[test]
    fn lcs() {
        assert_eq!(lcs_len(&toks("a b c d"), &toks("a x c d")), 3);
        assert_eq!(lcs_len(&toks("a b c"), &toks("")), 0);
    }

    #[test]
    fn table() {
        // (me, them, echo?)
        let cases: &[(&str, &str, bool)] = &[
            ("what would you do differently", "So what would you do differently next time?", true),
            ("what would you differently", "what would you do differently", true), // 4/4
            ("I think we should ship on Friday", "what would you do differently", false),
            ("would you do it", "what would you do differently", true), // 3/4 = 0.75
            ("I would do it", "what would you do differently", false),  // 2/4 = 0.5
            ("yeah", "yeah that sounds right", true),
            ("sounds right", "yeah that sounds right", true),
            ("right sounds", "yeah that sounds right", false), // 2 tokens must match in order
            ("okay", "yeah that sounds right", false),
            ("", "anything", false),
            ("hello there friend", "", false),
        ];
        for (me, them, want) in cases {
            assert_eq!(is_echo(&toks(me), &toks(them)), *want, "me={me:?} them={them:?}");
        }
    }

    #[test]
    fn me_after_them_is_flagged_immediately() {
        let mut d = EchoDetector::new();
        assert!(d.on_them_final("them-1-0", "Can you hear me okay", 1000, 2200).is_empty());
        assert!(d.on_me_final("me-1-1000", "can you hear me okay", 1100, 2300));
        // Outside the window: not an echo.
        assert!(!d.on_me_final("me-1-9000", "can you hear me okay", 9000, 10_000));
    }

    #[test]
    fn me_before_them_gets_patched() {
        let mut d = EchoDetector::new();
        assert!(!d.on_me_final("me-1-1100", "tell me about your last project", 1100, 2500));
        assert!(!d.on_me_final("me-1-5000", "sure so I built a compiler", 5000, 7000));
        let patched = d.on_them_final("them-1-1000", "Tell me about your last project.", 1000, 2400);
        assert_eq!(patched, vec!["me-1-1100".to_string()]);
        // Already patched: not reported again.
        assert!(d
            .on_them_final("them-1-2400", "Tell me about your last project.", 2400, 2600)
            .is_empty());
    }

    #[test]
    fn echo_spanning_two_them_finals() {
        let mut d = EchoDetector::new();
        d.on_them_final("t1", "what would you do", 1000, 1800);
        d.on_them_final("t2", "differently next time", 1800, 2600);
        assert!(d.on_me_final("m1", "would you do differently next", 1200, 2500));
    }

    #[test]
    fn old_segments_are_pruned() {
        let mut d = EchoDetector::new();
        d.on_them_final("t1", "hello there everyone", 0, 1000);
        d.on_me_final("m1", "unrelated words here", 100_000, 101_000);
        assert!(d.them.is_empty());
    }
}
