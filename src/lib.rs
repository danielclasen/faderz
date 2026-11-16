//! faderz: an operated multichannel mixer on the samplez engine.
//!
//! [`config`] reads the configuration and [`shape`] the mixer's shape;
//! [`state`] is the replicated mixer state, applied in log order on every
//! replica, and [`replica`] keeps it on the data directory as a set of one
//! voter. [`api`] serves the REST routes and [`ws`] the WebSocket of the
//! contract in `api/`, which [`contract`] compiles in; [`app`] holds what
//! they share, and [`ui`] serves the web UI built into the binary.
//! [`audio`] runs the samplez engine under the state: [`mix`] turns the
//! state into its gain matrix and [`meters`] its meters into meter frames.

pub mod api;
pub mod app;
pub mod audio;
pub mod config;
pub mod contract;
pub mod meters;
pub mod mix;
pub mod replica;
pub mod server;
pub mod shape;
pub mod state;
pub mod ui;
pub mod ws;
