//! The one error type every command returns. Serialized as `{ code, message, detail? }`
//! (docs/CONTRACT.md "Conventions"). The frontend shows `message` and branches on `code`.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{code}: {message}")]
pub struct AppError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Unexpected failure inside the app (not the user's fault, not retryable by them).
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new("invalid_argument", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message)
    }
}

impl From<tauri::Error> for AppError {
    fn from(e: tauri::Error) -> Self {
        AppError::new("tauri", "Window system error").with_detail(e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::new("io", "File system error").with_detail(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::new("json", "Invalid JSON").with_detail(e.to_string())
    }
}

impl From<tokio::task::JoinError> for AppError {
    fn from(e: tokio::task::JoinError) -> Self {
        AppError::internal("Background task failed").with_detail(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_camel_case_without_empty_detail() {
        let e = AppError::new("key_invalid", "Key rejected");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"code": "key_invalid", "message": "Key rejected"})
        );
        let e = e.with_detail("401");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["detail"], "401");
        assert_eq!(e.to_string(), "key_invalid: Key rejected");
    }
}
