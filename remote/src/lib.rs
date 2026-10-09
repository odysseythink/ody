//! Remote viewing/control of ody instances: a VPS gateway plus per-machine agents.
//!
//! - `proto`: JSON message envelopes shared by agents, the gateway, and browser clients.
//! - `gateway`: the VPS hub (auth, instance registry, websocket fan-out).
//! - `agent` (feature `agent`): per-machine bridge to local ody app-servers.

pub mod gateway;
pub mod proto;

use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;

/// Lock a std mutex, ignoring poisoning (a panicked writer must not cascade
/// into every subsequent reader).
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(feature = "agent")]
pub mod agent;
