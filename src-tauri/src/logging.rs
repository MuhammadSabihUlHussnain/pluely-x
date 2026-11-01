//! tracing + a daily rolling log file (7 kept) in the app log dir.
//!
//! Rules: never log API keys, request/response bodies, or transcript text (transcripts only at
//! debug level, which is off by default). `RUST_LOG`/`PLUELYX_LOG` override the filter.

use std::path::Path;

use serde::Deserialize;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder as RollingBuilder, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

use crate::error::{AppError, AppResult};

/// Default filter. Network crates are kept quiet so request headers (keys) never reach the log.
pub const DEFAULT_FILTER: &str = "info,pluelyx_lib=info,webview=info,tungstenite=info,\
tokio_tungstenite=info,reqwest=warn,hyper=warn,hyper_util=warn,h2=warn,rustls=warn,\
native_tls=warn,sqlx=warn,tao=warn,wry=warn";

/// Keeps the non-blocking writer alive; dropping it flushes the file.
pub struct LogGuard {
    _guard: Option<WorkerGuard>,
}

pub fn init(log_dir: &Path) -> LogGuard {
    let filter = std::env::var("PLUELYX_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .ok()
        .and_then(|f| EnvFilter::try_new(f).ok())
        .unwrap_or_else(|| EnvFilter::new(DEFAULT_FILTER));

    let file = std::fs::create_dir_all(log_dir).ok().and_then(|_| {
        RollingBuilder::new()
            .rotation(Rotation::DAILY)
            .filename_prefix("pluelyx")
            .filename_suffix("log")
            .max_log_files(7)
            .build(log_dir)
            .ok()
    });

    let (file_layer, guard) = match file {
        Some(appender) => {
            let (nb, guard) = tracing_appender::non_blocking(appender);
            (
                Some(
                    fmt::layer()
                        .with_ansi(false)
                        .with_target(true)
                        .with_writer(nb),
                ),
                Some(guard),
            )
        }
        None => (None, None),
    };
    let stderr_layer =
        cfg!(debug_assertions).then(|| fmt::layer().with_target(true).with_writer(std::io::stderr));

    // try_init also installs the log -> tracing bridge (tauri plugins use `log`).
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), dir = %log_dir.display(), "logging started");
    LogGuard { _guard: guard }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebLogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

/// Frontend log sink. The webview must not pass secrets or transcript text here.
#[tauri::command]
pub async fn log_write(level: WebLogLevel, message: String) -> AppResult<()> {
    if message.len() > 8_192 {
        return Err(AppError::invalid_argument(
            "Log message too long (max 8 KB)",
        ));
    }
    let message = redact(&message);
    match level {
        WebLogLevel::Error => tracing::error!(target: "webview", "{message}"),
        WebLogLevel::Warn => tracing::warn!(target: "webview", "{message}"),
        WebLogLevel::Info => tracing::info!(target: "webview", "{message}"),
        WebLogLevel::Debug => tracing::debug!(target: "webview", "{message}"),
    }
    Ok(())
}

/// Belt and braces: mask anything that looks like an Anthropic key before it hits the file.
pub fn redact(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("sk-ant-") {
        out.push_str(&rest[..i]);
        out.push_str("sk-ant-***");
        let tail = &rest[i + 7..];
        let end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_anthropic_keys() {
        assert_eq!(redact("key=sk-ant-api03-AbC_d-9 end"), "key=sk-ant-*** end");
        assert_eq!(redact("no key here"), "no key here");
        assert_eq!(redact("a sk-ant-x b sk-ant-y"), "a sk-ant-*** b sk-ant-***");
    }

    #[test]
    fn default_filter_parses() {
        assert!(EnvFilter::try_new(DEFAULT_FILTER).is_ok());
    }
}
