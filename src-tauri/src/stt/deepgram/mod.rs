//! Deepgram streaming speech-to-text (docs/NATIVE_DESIGN.md "Deepgram client").

pub mod backoff;
pub mod client;
pub mod config;
pub mod protocol;

#[cfg(test)]
pub mod mock;
