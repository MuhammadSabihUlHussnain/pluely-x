//! In-memory store of the most recent screenshots, by id. The LLM proxy substitutes
//! `@screenshot:<id>` from here, so image bytes never cross IPC on the way to the API.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::Serialize;

pub const CAPACITY: usize = 10;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotMeta {
    pub id: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub monitor: String,
    pub bytes: usize,
    pub thumb_data_url: String,
}

#[derive(Debug, Clone)]
pub struct Shot {
    pub meta: ScreenshotMeta,
    pub data: Arc<Vec<u8>>,
}

#[derive(Default)]
pub struct ScreenshotStore {
    shots: Mutex<VecDeque<Shot>>,
}

impl ScreenshotStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Shot>> {
        self.shots.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Insert, evicting the oldest beyond `CAPACITY`.
    pub fn insert(&self, shot: Shot) {
        let mut q = self.lock();
        q.retain(|s| s.meta.id != shot.meta.id);
        q.push_back(shot);
        while q.len() > CAPACITY {
            q.pop_front();
        }
    }

    pub fn get(&self, id: &str) -> Option<Shot> {
        self.lock().iter().find(|s| s.meta.id == id).cloned()
    }

    pub fn remove(&self, id: &str) -> bool {
        let mut q = self.lock();
        let before = q.len();
        q.retain(|s| s.meta.id != id);
        q.len() != before
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(id: &str) -> Shot {
        Shot {
            meta: ScreenshotMeta {
                id: id.into(),
                mime: "image/png".into(),
                width: 1,
                height: 1,
                source_width: 1,
                source_height: 1,
                monitor: "m".into(),
                bytes: 3,
                thumb_data_url: String::new(),
            },
            data: Arc::new(vec![1, 2, 3]),
        }
    }

    #[test]
    fn keeps_last_ten() {
        let s = ScreenshotStore::default();
        for i in 0..13 {
            s.insert(shot(&i.to_string()));
        }
        assert_eq!(s.len(), CAPACITY);
        assert!(s.get("0").is_none());
        assert!(s.get("2").is_none());
        assert!(s.get("3").is_some());
        assert!(s.get("12").is_some());
        assert!(s.remove("12"));
        assert!(!s.remove("12"));
        assert_eq!(s.len(), 9);
    }

    #[test]
    fn meta_is_camel_case() {
        let v = serde_json::to_value(&shot("a").meta).unwrap();
        assert!(v.get("sourceWidth").is_some());
        assert!(v.get("thumbDataUrl").is_some());
    }
}
