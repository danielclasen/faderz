//! faderz: an operated multichannel mixer on the samplez engine.
//!
//! [`shape`] is the mixer's shape and [`state`] the replicated mixer
//! state, applied in log order on every replica, checked against the
//! contract in `api/`, which [`contract`] compiles in.

pub mod contract;
pub mod shape;
pub mod state;
