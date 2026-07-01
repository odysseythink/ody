use crate::config::Config;
pub use ody_rollout::ARCHIVED_SESSIONS_SUBDIR;
pub use ody_rollout::Cursor;
pub use ody_rollout::INTERACTIVE_SESSION_SOURCES;
pub use ody_rollout::RolloutRecorder;
pub use ody_rollout::RolloutRecorderParams;
pub use ody_rollout::SESSIONS_SUBDIR;
pub use ody_rollout::SessionMeta;
pub use ody_rollout::SortDirection;
pub use ody_rollout::ThreadItem;
pub use ody_rollout::ThreadSortKey;
pub use ody_rollout::ThreadsPage;
pub use ody_rollout::append_thread_name;
pub use ody_rollout::find_archived_thread_path_by_id_str;
#[deprecated(note = "use find_thread_path_by_id_str")]
pub use ody_rollout::find_conversation_path_by_id_str;
pub use ody_rollout::find_thread_meta_by_name_str;
pub use ody_rollout::find_thread_name_by_id;
pub use ody_rollout::find_thread_names_by_ids;
pub use ody_rollout::find_thread_path_by_id_str;
pub use ody_rollout::parse_cursor;
pub use ody_rollout::read_head_for_summary;
pub use ody_rollout::read_session_meta_line;
pub use ody_rollout::rollout_date_parts;

impl ody_rollout::RolloutConfigView for Config {
    fn ody_home(&self) -> &std::path::Path {
        self.ody_home.as_path()
    }

    fn sqlite_home(&self) -> &std::path::Path {
        self.sqlite_home.as_path()
    }

    fn cwd(&self) -> &std::path::Path {
        self.cwd.as_path()
    }

    fn model_provider_id(&self) -> &str {
        self.model_provider_id.as_str()
    }

    fn generate_memories(&self) -> bool {
        self.memories.generate_memories
    }
}

pub(crate) mod list {
    pub use ody_rollout::find_thread_path_by_id_str;
}

#[cfg(test)]
pub(crate) mod recorder {
    pub use ody_rollout::RolloutRecorder;
}

pub(crate) use crate::session_rollout_init_error::map_session_init_error;

pub(crate) mod truncation {
    pub(crate) use crate::thread_rollout_truncation::*;
}
