use ody_api::SharedAuthProvider;
use ody_login::AuthManager;
use std::io;
use std::io::ErrorKind;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::info;

pub(super) struct RemoteControlConnectionAuth {
    pub(super) auth_provider: SharedAuthProvider,
    pub(super) account_id: String,
}

pub(super) async fn load_remote_control_auth(
    _auth_manager: &Arc<AuthManager>,
) -> io::Result<RemoteControlConnectionAuth> {
    Err(io::Error::new(
        ErrorKind::PermissionDenied,
        "remote control requires ChatGPT authentication; API key auth is not supported",
    ))
}

/// Placeholder for the removed `UnauthorizedRecovery` state machine.
///
/// Remote control enrollment requires ChatGPT backend auth, which has been
/// removed, so recovery never has any steps.
pub(super) struct RemoteControlAuthRecovery;

impl RemoteControlAuthRecovery {
    pub(super) fn has_next(&self) -> bool {
        false
    }

    pub(super) fn mode_name(&self) -> &'static str {
        "none"
    }

    pub(super) fn step_name(&self) -> &'static str {
        "done"
    }

    pub(super) fn unavailable_reason(&self) -> &'static str {
        "not_refreshable_auth"
    }
}

pub(super) async fn recover_remote_control_auth(
    _auth_recovery: &mut RemoteControlAuthRecovery,
    auth_change_rx: &mut watch::Receiver<u64>,
) -> bool {
    let _ = auth_change_rx;
    false
}

pub(super) fn mark_recovery_auth_change_seen(
    auth_change_rx: &mut watch::Receiver<u64>,
    auth_change_revision_before_recovery: u64,
) {
    let auth_change_revision_after_recovery = *auth_change_rx.borrow();
    if auth_change_revision_after_recovery == auth_change_revision_before_recovery.wrapping_add(1) {
        // Recovery updated the same watch that wakes the outer reconnect
        // loop. Mark only that single revision seen; if more revisions
        // arrived while recovery was in flight, leave them pending so the
        // reconnect loop still reacts to the later external auth change.
        auth_change_rx.borrow_and_update();
    }
}
