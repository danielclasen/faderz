//! faderz: an operated multichannel mixer on the samplez engine.
//!
//! [`config`] reads the configuration and [`shape`] the mixer's shape;
//! [`state`] is the replicated mixer state, applied in log order on every
//! replica, and [`replica`] keeps it on the data directory as a set of one
//! voter. [`api`] serves the REST routes and [`ws`] the WebSocket of the
//! contract in `api/`, which [`contract`] compiles in; [`app`] holds what
//! they share.

pub mod api;
pub mod app;
pub mod config;
pub mod contract;
pub mod replica;
pub mod server;
pub mod shape;
pub mod state;
pub mod ws;
