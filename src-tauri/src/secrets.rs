//! API key storage. Keys live in Windows Credential Manager (service `com.sabih.pluelyx`,
//! one entry per provider) and never cross into the webview.
//!
//! `keyring` MUST be built with `features = ["windows-native"]`: without it keyring v3 silently
//! falls back to an in-memory mock store and keys vanish on restart.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

pub const KEYRING_SERVICE: &str = "com.sabih.pluelyx";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Anthropic,
    Deepgram,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Anthropic, Provider::Deepgram];

    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::Deepgram => "deepgram",
        }
    }
}

pub trait SecretStore: Send + Sync {
    fn get(&self, p: Provider) -> AppResult<Option<String>>;
    fn set(&self, p: Provider, v: &str) -> AppResult<()>;
    fn delete(&self, p: Provider) -> AppResult<()>;
}

/// Managed state. `Secrets::get` is what other modules (LLM proxy, Deepgram client) call.
#[derive(Clone)]
pub struct Secrets(pub Arc<dyn SecretStore>);

impl Secrets {
    pub fn get(&self, p: Provider) -> AppResult<Option<String>> {
        self.0.get(p)
    }

    pub fn set(&self, p: Provider, v: &str) -> AppResult<()> {
        self.0.set(p, v)
    }

    pub fn delete(&self, p: Provider) -> AppResult<()> {
        self.0.delete(p)
    }
}

fn keyring_err(e: impl std::fmt::Display) -> AppError {
    AppError::new(
        "keyring_error",
        "Couldn't access Windows Credential Manager",
    )
    .with_detail(e.to_string())
}

/// Windows Credential Manager store with a small in-process cache (Credential Manager reads
/// are cheap but the LLM proxy calls `get` on every request).
pub struct KeyringStore {
    service: String,
    cache: Mutex<HashMap<Provider, Option<String>>>,
}

impl KeyringStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn entry(&self, p: Provider) -> AppResult<keyring::Entry> {
        keyring::Entry::new(&self.service, p.as_str()).map_err(keyring_err)
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<Provider, Option<String>>> {
        self.cache.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, p: Provider) -> AppResult<Option<String>> {
        if let Some(v) = self.cache().get(&p) {
            return Ok(v.clone());
        }
        let v = match self.entry(p)?.get_password() {
            Ok(v) => Some(v),
            Err(keyring::Error::NoEntry) => None,
            Err(e) => return Err(keyring_err(e)),
        };
        self.cache().insert(p, v.clone());
        Ok(v)
    }

    fn set(&self, p: Provider, v: &str) -> AppResult<()> {
        self.entry(p)?.set_password(v).map_err(keyring_err)?;
        self.cache().insert(p, Some(v.to_string()));
        Ok(())
    }

    fn delete(&self, p: Provider) -> AppResult<()> {
        match self.entry(p)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(keyring_err(e)),
        }
        self.cache().insert(p, None);
        Ok(())
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemStore {
    map: Mutex<HashMap<Provider, String>>,
}

impl SecretStore for MemStore {
    fn get(&self, p: Provider) -> AppResult<Option<String>> {
        Ok(self
            .map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&p)
            .cloned())
    }

    fn set(&self, p: Provider, v: &str) -> AppResult<()> {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(p, v.to_string());
        Ok(())
    }

    fn delete(&self, p: Provider) -> AppResult<()> {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&p);
        Ok(())
    }
}

/// `"…a1b2"` — the last 4 characters, for display only.
pub fn key_hint(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    format!("\u{2026}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memstore_roundtrip() {
        let s = Secrets(Arc::new(MemStore::default()));
        assert_eq!(s.get(Provider::Anthropic).unwrap(), None);
        s.set(Provider::Anthropic, "sk-ant-test-1234").unwrap();
        assert_eq!(
            s.get(Provider::Anthropic).unwrap().as_deref(),
            Some("sk-ant-test-1234")
        );
        assert_eq!(s.get(Provider::Deepgram).unwrap(), None);
        s.delete(Provider::Anthropic).unwrap();
        assert_eq!(s.get(Provider::Anthropic).unwrap(), None);
        // deleting a missing key is fine
        s.delete(Provider::Deepgram).unwrap();
    }

    #[test]
    fn provider_wire_names() {
        assert_eq!(
            serde_json::to_string(&Provider::Anthropic).unwrap(),
            "\"anthropic\""
        );
        let p: Provider = serde_json::from_str("\"deepgram\"").unwrap();
        assert_eq!(p, Provider::Deepgram);
        assert_eq!(Provider::Deepgram.as_str(), "deepgram");
    }

    #[test]
    fn hint_is_last_four() {
        assert_eq!(key_hint("sk-ant-abcdefa1b2"), "\u{2026}a1b2");
        assert_eq!(key_hint("ab"), "\u{2026}ab");
    }

    /// Real Credential Manager round-trip with a throwaway service name.
    #[test]
    #[ignore]
    fn keyring_roundtrip_hardware() {
        let store = KeyringStore::new("com.sabih.pluelyx.test");
        store.set(Provider::Deepgram, "dummy-value").unwrap();
        let fresh = KeyringStore::new("com.sabih.pluelyx.test");
        assert_eq!(
            fresh.get(Provider::Deepgram).unwrap().as_deref(),
            Some("dummy-value")
        );
        fresh.delete(Provider::Deepgram).unwrap();
        assert_eq!(
            KeyringStore::new("com.sabih.pluelyx.test")
                .get(Provider::Deepgram)
                .unwrap(),
            None
        );
    }
}
