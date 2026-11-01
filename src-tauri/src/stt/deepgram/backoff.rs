//! Reconnect policy (pure): full-jitter exponential backoff, the capture-restart ladder, and
//! fatal vs retryable classification of Deepgram failures.

use std::time::Duration;

/// Full-jitter exponential backoff: delay ∈ [floor, min(cap, base·2ⁿ)].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackoffConfig {
    pub base: Duration,
    pub cap: Duration,
    /// A connection that stayed up this long resets the attempt counter.
    pub healthy_reset: Duration,
    /// Never retry sooner than this (avoids a hot loop on a 0 ms draw).
    pub floor: Duration,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            base: Duration::from_millis(250),
            cap: Duration::from_secs(10),
            healthy_reset: Duration::from_secs(30),
            floor: Duration::from_millis(50),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Backoff {
    cfg: BackoffConfig,
    attempt: u32,
    rng: fastrand::Rng,
}

impl Backoff {
    pub fn new(cfg: BackoffConfig) -> Self {
        Self {
            cfg,
            attempt: 0,
            rng: fastrand::Rng::new(),
        }
    }

    pub fn with_seed(cfg: BackoffConfig, seed: u64) -> Self {
        Self {
            cfg,
            attempt: 0,
            rng: fastrand::Rng::with_seed(seed),
        }
    }

    /// Consecutive failures so far.
    #[cfg(test)]
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Upper bound for the next delay.
    pub fn ceiling(&self) -> Duration {
        let shift = self.attempt.min(20);
        let ms = (self.cfg.base.as_millis() as u64).saturating_mul(1u64 << shift);
        Duration::from_millis(ms.min(self.cfg.cap.as_millis() as u64))
    }

    /// The next delay; advances the attempt counter.
    pub fn next_delay(&mut self) -> Duration {
        let ceil = self.ceiling().as_millis() as u64;
        self.attempt = self.attempt.saturating_add(1);
        let ms = self.rng.u64(0..=ceil);
        Duration::from_millis(ms).max(self.cfg.floor)
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// Reset if the connection that just ended had been healthy long enough.
    pub fn note_connection_lasted(&mut self, lasted: Duration) {
        if lasted >= self.cfg.healthy_reset {
            self.reset();
        }
    }
}

/// Deterministic capture-restart ladder: 100 ms → 500 ms → 1 s → 2 s → 5 s (cap).
#[derive(Debug, Clone, Default)]
pub struct Ladder {
    step: usize,
}

impl Ladder {
    pub const STEPS_MS: [u64; 5] = [100, 500, 1_000, 2_000, 5_000];

    pub fn next_delay(&mut self) -> Duration {
        let ms = Self::STEPS_MS[self.step.min(Self::STEPS_MS.len() - 1)];
        self.step = self.step.saturating_add(1);
        Duration::from_millis(ms)
    }

    pub fn reset(&mut self) {
        self.step = 0;
    }
}

/// How a connection attempt or a live socket failed.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// Don't retry (bad key, no credit, rejected request).
    Fatal { code: &'static str, message: String },
    /// Retry after backoff (at least `min_delay`).
    Retry {
        code: &'static str,
        message: String,
        min_delay: Option<Duration>,
    },
}

#[cfg_attr(not(test), allow(dead_code))]
impl Failure {
    pub fn code(&self) -> &'static str {
        match self {
            Failure::Fatal { code, .. } | Failure::Retry { code, .. } => code,
        }
    }

    pub fn is_fatal(&self) -> bool {
        matches!(self, Failure::Fatal { .. })
    }

    pub fn retry(code: &'static str, message: impl Into<String>) -> Self {
        Failure::Retry {
            code,
            message: message.into(),
            min_delay: None,
        }
    }

    pub fn fatal(code: &'static str, message: impl Into<String>) -> Self {
        Failure::Fatal {
            code,
            message: message.into(),
        }
    }
}

/// Minimum wait after a 429.
pub const RATE_LIMIT_MIN: Duration = Duration::from_secs(5);

/// Handshake HTTP status → failure class.
pub fn classify_http_status(status: u16, rate_limit_min: Duration) -> Failure {
    match status {
        401 | 403 => Failure::fatal("dg_auth", "Deepgram rejected the API key."),
        402 => Failure::fatal(
            "dg_payment",
            "Deepgram says the account is out of credit.",
        ),
        429 => Failure::Retry {
            code: "dg_rate_limited",
            message: "Deepgram is rate limiting this key; retrying.".into(),
            min_delay: Some(rate_limit_min),
        },
        408 => Failure::retry("dg_timeout", "Deepgram timed out; retrying."),
        400..=499 => Failure::fatal(
            "dg_bad_request",
            format!("Deepgram rejected the request (HTTP {status})."),
        ),
        500..=599 => Failure::retry(
            "dg_server",
            format!("Deepgram had a server error (HTTP {status}); retrying."),
        ),
        _ => Failure::retry(
            "dg_network",
            format!("Unexpected response from Deepgram (HTTP {status}); retrying."),
        ),
    }
}

/// Close frame from Deepgram (while we weren't stopping) → failure class.
pub fn classify_close(code: u16, reason: &str) -> Failure {
    if reason.contains("NET-0001") {
        return Failure::retry("dg_timeout", "Deepgram closed an idle stream; reconnecting.");
    }
    match code {
        1008 => Failure::fatal(
            "dg_bad_request",
            format!("Deepgram couldn't decode the audio ({}).", short(reason)),
        ),
        1011 => Failure::retry("dg_server", "Deepgram closed the stream (server error); reconnecting."),
        _ => Failure::retry(
            "dg_network",
            format!("Deepgram closed the connection (code {code}); reconnecting."),
        ),
    }
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        "no reason given".into()
    } else {
        s.chars().take(120).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_jitter_bounds_grow_to_cap() {
        let cfg = BackoffConfig::default();
        let mut b = Backoff::with_seed(cfg, 7);
        let ceilings: Vec<u64> = (0..8)
            .map(|_| {
                let c = b.ceiling().as_millis() as u64;
                let d = b.next_delay();
                assert!(d >= cfg.floor && d.as_millis() as u64 <= c.max(50), "{d:?} > {c}");
                c
            })
            .collect();
        assert_eq!(ceilings, vec![250, 500, 1000, 2000, 4000, 8000, 10000, 10000]);
        assert_eq!(b.attempt(), 8);
    }

    #[test]
    fn jitter_spreads_and_seed_is_deterministic() {
        let draw = |seed: u64| {
            let mut b = Backoff::with_seed(BackoffConfig::default(), seed);
            for _ in 0..6 {
                b.next_delay();
            }
            b.next_delay() // ceiling 10 s
        };
        assert_eq!(draw(42), draw(42));
        let distinct: std::collections::HashSet<Duration> = (0..50).map(draw).collect();
        assert!(distinct.len() > 10, "jitter should vary: {distinct:?}");
    }

    #[test]
    fn reset_after_healthy_connection() {
        let mut b = Backoff::with_seed(BackoffConfig::default(), 3);
        for _ in 0..5 {
            b.next_delay();
        }
        b.note_connection_lasted(Duration::from_secs(5));
        assert_eq!(b.attempt(), 5, "short-lived connection doesn't reset");
        b.note_connection_lasted(Duration::from_secs(31));
        assert_eq!(b.attempt(), 0);
        assert_eq!(b.ceiling(), Duration::from_millis(250));
    }

    #[test]
    fn ladder() {
        let mut l = Ladder::default();
        let v: Vec<u64> = (0..7).map(|_| l.next_delay().as_millis() as u64).collect();
        assert_eq!(v, vec![100, 500, 1000, 2000, 5000, 5000, 5000]);
        l.reset();
        assert_eq!(l.next_delay(), Duration::from_millis(100));
    }

    #[test]
    fn http_classification() {
        let m = RATE_LIMIT_MIN;
        assert_eq!(classify_http_status(401, m).code(), "dg_auth");
        assert!(classify_http_status(401, m).is_fatal());
        assert!(classify_http_status(403, m).is_fatal());
        assert_eq!(classify_http_status(402, m).code(), "dg_payment");
        assert!(classify_http_status(402, m).is_fatal());
        assert_eq!(classify_http_status(400, m).code(), "dg_bad_request");
        assert!(classify_http_status(400, m).is_fatal());
        assert!(classify_http_status(404, m).is_fatal());
        match classify_http_status(429, m) {
            Failure::Retry { code, min_delay, .. } => {
                assert_eq!(code, "dg_rate_limited");
                assert_eq!(min_delay, Some(Duration::from_secs(5)));
            }
            f => panic!("{f:?}"),
        }
        for s in [500, 502, 503, 504] {
            let f = classify_http_status(s, m);
            assert!(!f.is_fatal());
            assert_eq!(f.code(), "dg_server");
        }
        assert_eq!(classify_http_status(408, m).code(), "dg_timeout");
    }

    #[test]
    fn close_classification() {
        assert_eq!(classify_close(1011, "NET-0001 no audio").code(), "dg_timeout");
        assert!(!classify_close(1011, "NET-0001").is_fatal());
        assert_eq!(classify_close(1011, "").code(), "dg_server");
        assert!(!classify_close(1006, "").is_fatal());
        assert_eq!(classify_close(1006, "").code(), "dg_network");
        assert!(classify_close(1008, "DATA-0000").is_fatal());
        assert_eq!(classify_close(1008, "DATA-0000").code(), "dg_bad_request");
        assert!(!classify_close(1000, "").is_fatal());
    }
}
