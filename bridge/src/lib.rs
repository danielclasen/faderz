//! faderz-bridge: a faderz client that drives Mackie Control surfaces.

pub mod auth;
pub mod bridge;
pub mod config;
pub mod emulator;
pub mod engine;
#[cfg(test)]
mod fixtures;
pub mod mcu;
pub mod midi;
pub mod mirror;
pub mod session;
pub mod surface;
pub mod taper;
