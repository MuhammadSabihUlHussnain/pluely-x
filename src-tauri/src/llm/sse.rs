//! Incremental Server-Sent Events parser (pure). Feed raw byte chunks as they arrive; complete
//! events come out. Handles `\n` and `\r\n` line endings, multi-line `data:`, comments, and
//! chunks that split lines or UTF-8 sequences anywhere.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

/// Cap on a single unterminated line, so a broken stream can't grow memory without bound.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut start = 0;
        while let Some(pos) = self.buf[start..].iter().position(|&b| b == b'\n') {
            let end = start + pos;
            let mut line = &self.buf[start..end];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            let line = String::from_utf8_lossy(line).into_owned();
            if let Some(ev) = self.line(&line) {
                out.push(ev);
            }
            start = end + 1;
        }
        self.buf.drain(..start);
        if self.buf.len() > MAX_LINE {
            return Err("SSE line too long".into());
        }
        Ok(out)
    }

    /// End of stream: dispatch a trailing event that wasn't followed by a blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            let line = String::from_utf8_lossy(&rest)
                .trim_end_matches('\r')
                .to_string();
            if let Some(ev) = self.line(&line) {
                return Some(ev);
            }
        }
        self.dispatch()
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() {
            self.event = None;
            return None;
        }
        Some(SseEvent {
            event: self.event.take(),
            data: std::mem::take(&mut self.data).join("\n"),
        })
    }

    fn line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None; // comment
        }
        let (field, value) = match line.find(':') {
            Some(i) => {
                let v = &line[i + 1..];
                (&line[..i], v.strip_prefix(' ').unwrap_or(v))
            }
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {} // id, retry, unknown
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(chunks: &[&[u8]]) -> Vec<SseEvent> {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        for c in chunks {
            out.extend(p.push(c).unwrap());
        }
        out.extend(p.finish());
        out
    }

    #[test]
    fn basic_events() {
        let ev = all(&[b"event: ping\ndata: {\"type\": \"ping\"}\n\nevent: x\ndata: 1\n\n"]);
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].event.as_deref(), Some("ping"));
        assert_eq!(ev[0].data, "{\"type\": \"ping\"}");
        assert_eq!(ev[1].data, "1");
    }

    #[test]
    fn split_anywhere_and_crlf() {
        let s = "event: message_start\r\ndata: {\"a\":\"h\u{e9}llo\"}\r\n\r\n".as_bytes();
        for split in 0..s.len() {
            let ev = all(&[&s[..split], &s[split..]]);
            assert_eq!(ev.len(), 1, "split at {split}");
            assert_eq!(ev[0].data, "{\"a\":\"h\u{e9}llo\"}");
            assert_eq!(ev[0].event.as_deref(), Some("message_start"));
        }
        // byte-by-byte
        let chunks: Vec<&[u8]> = s.chunks(1).collect();
        assert_eq!(all(&chunks).len(), 1);
    }

    #[test]
    fn multiline_data_comments_and_no_space() {
        let ev = all(&[b": keepalive\ndata:a\ndata: b\nid: 7\nretry: 10\n\n"]);
        assert_eq!(
            ev,
            vec![SseEvent {
                event: None,
                data: "a\nb".into()
            }]
        );
    }

    #[test]
    fn blank_lines_without_data_emit_nothing_and_trailing_event_flushes() {
        assert!(all(&[b"\n\n\nevent: x\n\n"]).is_empty());
        let ev = all(&[b"data: last"]);
        assert_eq!(
            ev,
            vec![SseEvent {
                event: None,
                data: "last".into()
            }]
        );
    }
}
