//! Deepgram `/v1/listen` URL builder (pure).

use crate::audio::frame::SAMPLE_RATE;
use crate::audio::Source;

pub const DEFAULT_BASE_URL: &str = "wss://api.deepgram.com/v1/listen";
pub const MODEL: &str = "nova-3";
/// Deepgram requires `utterance_end_ms` ≥ 1000.
pub const MIN_UTTERANCE_END_MS: u32 = 1_000;
/// Cap on keyterms sent (Deepgram limits the total keyterm prompt size).
pub const MAX_KEYTERMS: usize = 100;
pub const MAX_KEYTERM_LEN: usize = 100;

/// Streaming parameters for one source's socket.
#[derive(Debug, Clone, PartialEq)]
pub struct ListenParams {
    pub language: String,
    pub endpointing_ms: u32,
    pub utterance_end_ms: u32,
    pub keyterms: Vec<String>,
    pub diarize: bool,
    pub source: Source,
}

/// RFC 3986 percent-encoding of everything but unreserved characters.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Trimmed, de-duplicated, non-empty keyterms within Deepgram's limits.
pub fn clean_keyterms(terms: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    terms
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().take(MAX_KEYTERM_LEN).collect::<String>())
        .filter(|t| seen.insert(t.to_lowercase()))
        .take(MAX_KEYTERMS)
        .collect()
}

/// `base?model=nova-3&language=..&encoding=linear16&sample_rate=16000&channels=1&...`
pub fn build_url(base: &str, p: &ListenParams) -> String {
    let language = if p.language.trim().is_empty() {
        "en".to_string()
    } else {
        p.language.trim().to_string()
    };
    let mut q: Vec<(&str, String)> = vec![
        ("model", MODEL.into()),
        ("language", language),
        ("encoding", "linear16".into()),
        ("sample_rate", SAMPLE_RATE.to_string()),
        ("channels", "1".into()),
        ("interim_results", "true".into()),
        ("smart_format", "true".into()),
        ("punctuate", "true".into()),
        ("endpointing", p.endpointing_ms.max(10).to_string()),
        (
            "utterance_end_ms",
            p.utterance_end_ms.max(MIN_UTTERANCE_END_MS).to_string(),
        ),
        ("vad_events", "true".into()),
        ("tag", format!("pluelyx-{}", p.source.as_str())),
    ];
    if p.diarize {
        q.push(("diarize", "true".into()));
    }
    for t in clean_keyterms(&p.keyterms) {
        q.push(("keyterm", t));
    }
    let query = q
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let sep = if base.contains('?') { '&' } else { '?' };
    format!("{base}{sep}{query}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> ListenParams {
        ListenParams {
            language: "en".into(),
            endpointing_ms: 300,
            utterance_end_ms: 1000,
            keyterms: vec![],
            diarize: false,
            source: Source::Me,
        }
    }

    #[test]
    fn matches_the_design_url() {
        assert_eq!(
            build_url(DEFAULT_BASE_URL, &params()),
            "wss://api.deepgram.com/v1/listen?model=nova-3&language=en&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&smart_format=true&punctuate=true&endpointing=300&utterance_end_ms=1000&vad_events=true&tag=pluelyx-me"
        );
    }

    #[test]
    fn diarize_and_keyterms_are_encoded() {
        let mut p = params();
        p.source = Source::Them;
        p.diarize = true;
        p.keyterms = vec![
            "Kubernetes".into(),
            "  Muhammad Sabih  ".into(),
            "".into(),
            "kubernetes".into(),
            "C++ & Rust?".into(),
        ];
        let url = build_url("ws://127.0.0.1:9/v1/listen", &p);
        assert!(url.starts_with("ws://127.0.0.1:9/v1/listen?model=nova-3&"));
        assert!(url.contains("&tag=pluelyx-them&diarize=true&keyterm=Kubernetes&keyterm=Muhammad%20Sabih&keyterm=C%2B%2B%20%26%20Rust%3F"), "{url}");
        assert_eq!(url.matches("keyterm=").count(), 3, "deduped case-insensitively");
    }

    #[test]
    fn clamps_utterance_end_and_defaults_language() {
        let mut p = params();
        p.utterance_end_ms = 500;
        p.language = " ".into();
        let url = build_url(DEFAULT_BASE_URL, &p);
        assert!(url.contains("utterance_end_ms=1000"));
        assert!(url.contains("language=en&"));
    }

    #[test]
    fn keyterm_limits() {
        let terms: Vec<String> = (0..500).map(|i| format!("term{i}")).collect();
        assert_eq!(clean_keyterms(&terms).len(), MAX_KEYTERMS);
        let long = "x".repeat(1000);
        assert_eq!(clean_keyterms(&[long])[0].len(), MAX_KEYTERM_LEN);
        assert_eq!(percent_encode("é"), "%C3%A9");
    }
}
