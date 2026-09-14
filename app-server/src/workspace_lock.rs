//! Multi-window write-intent registry (E4). Memory-only: a crashed or
//! disconnected client's hold must not outlive its connection.
//! Serialization keys already serialize same-key requests; this guards
//! *cross-connection* interleaving that keys cannot see.

//! Multi-window write-intent registry (E4). Memory-only: a crashed or
//! disconnected client's hold must not outlive its connection.
//! Serialization keys already serialize same-key requests; this guards
//! *cross-connection* interleaving that keys cannot see.

use std::collections::HashMap;
use std::sync::Mutex;

use ody_app_server_protocol::JSONRPCErrorError;

use crate::error_code::invalid_params;
use crate::outgoing_message::ConnectionId;

#[derive(Default)]
pub(crate) struct WorkspaceWriteLock {
    holders: Mutex<HashMap<String, ConnectionId>>,
}

impl WorkspaceWriteLock {
    /// Explicit writer declaration. Idempotent for the current holder;
    /// rejects other connections.
    pub(crate) fn acquire(
        &self,
        project_id: &str,
        connection_id: ConnectionId,
    ) -> Result<(), JSONRPCErrorError> {
        let mut holders = self.holders.lock().expect("lock table");
        match holders.get(project_id) {
            Some(holder) if *holder == connection_id => Ok(()),
            Some(_) => Err(invalid_params(format!(
                "project {project_id} is locked by another session; \
                 wait for it to finish or ask the other window to release"
            ))),
            None => {
                holders.insert(project_id.to_owned(), connection_id);
                Ok(())
            }
        }
    }

    /// Owner-only release; non-owner call is a no-op (not an error).
    pub(crate) fn release(&self, project_id: &str, connection_id: ConnectionId) {
        let mut holders = self.holders.lock().expect("lock table");
        if holders.get(project_id) == Some(&connection_id) {
            holders.remove(project_id);
        }
    }

    pub(crate) fn holder(&self, project_id: &str) -> Option<ConnectionId> {
        self.holders.lock().expect("lock table").get(project_id).copied()
    }

    /// Write-path guard used by mutating operations: passes when no holder
    /// or the caller holds the lock; rejects other holders.
    pub(crate) fn check(
        &self,
        project_id: &str,
        connection_id: ConnectionId,
    ) -> Result<(), JSONRPCErrorError> {
        match self.holder(project_id) {
            Some(holder) if holder != connection_id => self.acquire(project_id, connection_id),
            _ => Ok(()),
        }
    }

    pub(crate) fn connection_closed(&self, connection_id: ConnectionId) {
        self.holders
            .lock()
            .expect("lock table")
            .retain(|_, holder| *holder != connection_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_conflict_rejects_second_holder_and_release_is_owner_only() {
        let lock = WorkspaceWriteLock::default();
        lock.acquire("p1", ConnectionId(1)).expect("first acquire");
        let err = lock.acquire("p1", ConnectionId(2)).expect_err("second holder rejected");
        assert!(err.message.contains("locked by another session"), "got: {}", err.message);
        assert!(err.message.contains("p1"));
        lock.release("p1", ConnectionId(2)); // non-owner release is a no-op
        assert_eq!(lock.holder("p1"), Some(ConnectionId(1)));
        lock.release("p1", ConnectionId(1));
        assert_eq!(lock.holder("p1"), None);
        lock.acquire("p1", ConnectionId(2)).expect("acquire after release");
    }

    #[test]
    fn reacquire_by_same_holder_is_idempotent() {
        let lock = WorkspaceWriteLock::default();
        lock.acquire("p1", ConnectionId(1)).unwrap();
        lock.acquire("p1", ConnectionId(1)).expect("same holder re-acquire is a no-op");
        lock.release("p1", ConnectionId(1));
        assert_eq!(lock.holder("p1"), None);
    }

    #[test]
    fn connection_closed_releases_every_held_project() {
        let lock = WorkspaceWriteLock::default();
        lock.acquire("p1", ConnectionId(1)).unwrap();
        lock.acquire("p2", ConnectionId(1)).unwrap();
        lock.acquire("p3", ConnectionId(2)).unwrap();
        lock.connection_closed(ConnectionId(1));
        assert_eq!(lock.holder("p1"), None);
        assert_eq!(lock.holder("p2"), None);
        assert_eq!(lock.holder("p3"), Some(ConnectionId(2)), "other connection untouched");
    }
}
