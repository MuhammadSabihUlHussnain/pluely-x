//! macOS audio backend (macOS 14.4+).
//!
//! - `me`: the microphone through CoreAudio (cpal), default input or a chosen device.
//! - `them`: a global Core Audio *process tap* (everything any app plays, whichever output
//!   it plays on) read through a private aggregate device, the approach original Pluely
//!   uses. Because it isn't tied to one output device, the "call plays on another device"
//!   problem from Windows doesn't exist here.
//! - Device lists come from Core Audio; changes are found by polling ([`control`]).
//!
//! Both captures run on their own thread and feed the shared [`crate::audio::pump`], so
//! framing, levels and tail-fill behave exactly as on Windows.

pub mod capture;
pub mod control;
pub mod devices;
