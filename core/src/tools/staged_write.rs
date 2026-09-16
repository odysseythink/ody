//! Host-installed sink for staged workspace writes (工程模式 Canvas P1 切片1).
//!
//! When the host (app-server) runs a workspace-bound thread, it installs a
//! [`StagedWriteSink`] via the thread extension init (or session extension
//! data). Core's direct file-write tools (`write_file`, `edit_file`,
//! `apply_patch`) notify the sink after a write commits successfully, so the
//! host can accumulate reversible workspace changesets without core knowing
//! about workspaces. When no sink is installed, notification is a no-op.
//!
//! The notification carries the *base* (pre-write) content because that is
//! only available in-process at write time; hosts must not need to reconstruct
//! it from disk or diffs afterwards.

use std::path::PathBuf;
use std::sync::Arc;

use crate::session::session::Session;

/// One committed file write observed by the sink.
#[derive(Clone, Debug, PartialEq)]
pub struct StagedWrite {
    pub path: PathBuf,
    /// Content before the write; `None` when the file did not exist.
    pub old_content: Option<String>,
    /// Content after the write; `None` when the write deleted the file.
    pub new_content: Option<String>,
}

/// Host hook that receives staged workspace writes.
pub trait StagedWriteSink: Send + Sync {
    /// Called synchronously after a file write committed successfully.
    /// Implementations must be cheap; defer heavy work to a spawned task.
    fn staged_write(&self, session_id: &str, write: StagedWrite);
}

/// Returns the sink installed for this session, if any.
///
/// Looks at session extension data first (mutable, test-friendly), then at
/// the host-supplied thread extension init (app-server injection vector).
fn session_staged_write_sink(
    session: &Session,
) -> Option<Arc<dyn StagedWriteSink>> {
    session
        .services
        .session_extension_data
        .get::<Arc<dyn StagedWriteSink>>()
        .map(|sink| (*sink).clone())
        .or_else(|| {
            session
                .services
                .mcp_thread_init
                .get::<Arc<dyn StagedWriteSink>>()
                .map(|sink| (*sink).clone())
        })
}

/// Notifies the installed sink (if any) about a committed file write.
pub(crate) fn notify_staged_write(
    session: &Session,
    path: PathBuf,
    old_content: Option<String>,
    new_content: Option<String>,
) {
    if let Some(sink) = session_staged_write_sink(session) {
        sink.staged_write(
            &session.thread_id.to_string(),
            StagedWrite {
                path,
                old_content,
                new_content,
            },
        );
    }
}

/// Notifies the sink (if any) once per committed change in an apply_patch delta.
///
/// `AppliedPatchChange` carries full old/new content for every variant, so the
/// host receives the same shape of notification as for write_file/edit_file.
pub(crate) fn notify_staged_writes_from_changes(
    session: &Session,
    changes: &[ody_apply_patch::AppliedPatchChange],
) {
    for change in changes {
        let (old_content, new_content) = match &change.change {
            ody_apply_patch::AppliedPatchFileChange::Add {
                content,
                overwritten_content,
            } => (overwritten_content.clone(), Some(content.clone())),
            ody_apply_patch::AppliedPatchFileChange::Delete { content } => {
                (Some(content.clone()), None)
            }
            ody_apply_patch::AppliedPatchFileChange::Update {
                old_content,
                new_content,
                ..
            } => (Some(old_content.clone()), Some(new_content.clone())),
        };
        notify_staged_write(session, change.path.clone(), old_content, new_content);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::make_session_and_context_with_rx;
    use std::sync::Mutex;

    struct RecordingSink {
        writes: Mutex<Vec<(String, StagedWrite)>>,
    }

    impl RecordingSink {
        fn new() -> Self {
            Self {
                writes: Mutex::new(Vec::new()),
            }
        }

        fn recorded(&self) -> Vec<(String, StagedWrite)> {
            self.writes.lock().expect("lock").clone()
        }
    }

    impl StagedWriteSink for RecordingSink {
        fn staged_write(&self, session_id: &str, write: StagedWrite) {
            self.writes
                .lock()
                .expect("lock")
                .push((session_id.to_string(), write));
        }
    }

    #[tokio::test]
    async fn session_extension_data_sink_receives_notification() {
        let (session, _turn, _rx) = make_session_and_context_with_rx().await;
        let sink = Arc::new(RecordingSink::new());
        session
            .services
            .session_extension_data
            .insert::<Arc<dyn StagedWriteSink>>(sink.clone());

        notify_staged_write(
            &session,
            PathBuf::from("/tmp/example.txt"),
            Some("old\n".to_string()),
            Some("new\n".to_string()),
        );

        let recorded = sink.recorded();
        assert_eq!(recorded.len(), 1);
        let (session_id, write) = &recorded[0];
        assert_eq!(session_id, &session.thread_id.to_string());
        assert_eq!(write.path, PathBuf::from("/tmp/example.txt"));
        assert_eq!(write.old_content.as_deref(), Some("old\n"));
        assert_eq!(write.new_content.as_deref(), Some("new\n"));
    }

    #[tokio::test]
    async fn notification_without_sink_is_a_no_op() {
        let (session, _turn, _rx) = make_session_and_context_with_rx().await;
        // No sink installed: must not panic.
        notify_staged_write(
            &session,
            PathBuf::from("/tmp/example.txt"),
            None,
            Some("new\n".to_string()),
        );
    }

    fn applied_add(path: &str, content: &str, overwritten: Option<&str>) -> ody_apply_patch::AppliedPatchChange {
        ody_apply_patch::AppliedPatchChange {
            path: PathBuf::from(path),
            change: ody_apply_patch::AppliedPatchFileChange::Add {
                content: content.to_string(),
                overwritten_content: overwritten.map(str::to_string),
            },
        }
    }

    fn applied_delete(path: &str, content: &str) -> ody_apply_patch::AppliedPatchChange {
        ody_apply_patch::AppliedPatchChange {
            path: PathBuf::from(path),
            change: ody_apply_patch::AppliedPatchFileChange::Delete {
                content: content.to_string(),
            },
        }
    }

    fn applied_update(
        path: &str,
        old_content: &str,
        new_content: &str,
    ) -> ody_apply_patch::AppliedPatchChange {
        ody_apply_patch::AppliedPatchChange {
            path: PathBuf::from(path),
            change: ody_apply_patch::AppliedPatchFileChange::Update {
                move_path: None,
                old_content: old_content.to_string(),
                overwritten_move_content: None,
                new_content: new_content.to_string(),
            },
        }
    }

    #[tokio::test]
    async fn apply_patch_changes_notify_with_full_contents() {
        let (session, _turn, _rx) = make_session_and_context_with_rx().await;
        let sink = Arc::new(RecordingSink::new());
        session
            .services
            .session_extension_data
            .insert::<Arc<dyn StagedWriteSink>>(sink.clone());

        let changes = vec![
            applied_add("new.txt", "added\n", None),
            applied_update("mod.txt", "before\n", "after\n"),
            applied_delete("gone.txt", "deleted\n"),
        ];
        notify_staged_writes_from_changes(&session, &changes);

        let recorded = sink.recorded();
        assert_eq!(recorded.len(), 3);
        let writes: Vec<&StagedWrite> = recorded.iter().map(|(_, write)| write).collect();
        assert_eq!(
            writes[0],
            &StagedWrite {
                path: PathBuf::from("new.txt"),
                old_content: None,
                new_content: Some("added\n".to_string()),
            }
        );
        assert_eq!(
            writes[1],
            &StagedWrite {
                path: PathBuf::from("mod.txt"),
                old_content: Some("before\n".to_string()),
                new_content: Some("after\n".to_string()),
            }
        );
        assert_eq!(
            writes[2],
            &StagedWrite {
                path: PathBuf::from("gone.txt"),
                old_content: Some("deleted\n".to_string()),
                new_content: None,
            }
        );
    }

    #[tokio::test]
    async fn apply_patch_overwritten_add_reports_overwritten_base() {
        let (session, _turn, _rx) = make_session_and_context_with_rx().await;
        let sink = Arc::new(RecordingSink::new());
        session
            .services
            .session_extension_data
            .insert::<Arc<dyn StagedWriteSink>>(sink.clone());

        notify_staged_writes_from_changes(&session, &[applied_add("clobber.txt", "new\n", Some("orig\n"))]);

        let recorded = sink.recorded();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].1.old_content.as_deref(), Some("orig\n"));
        assert_eq!(recorded[0].1.new_content.as_deref(), Some("new\n"));
    }
}
