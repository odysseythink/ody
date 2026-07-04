use ody_utils_absolute_path::AbsolutePathBuf;
use std::path::Path;
use std::path::PathBuf;
use tokio::sync::Mutex;

/// Session-level plan file artifact. Lives in Plan mode only.
#[derive(Debug)]
pub struct PlanArtifact {
    state: Mutex<PlanArtifactState>,
    ody_home: AbsolutePathBuf,
    thread_id: ody_protocol::ThreadId,
    date: String,
}

#[derive(Debug, PartialEq)]
pub enum PlanArtifactState {
    Temporary { temp_path: PathBuf },
    Finalized { final_path: PathBuf },
    InlineOnly,
}

#[derive(Debug)]
pub enum PlanWriteOutcome {
    Written { path: PathBuf },
    InlineOnly,
    #[allow(dead_code)]
    Failed { error: PlanArtifactError },
}

#[derive(Debug, thiserror::Error)]
pub enum PlanArtifactError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid slug")]
    #[allow(dead_code)]
    InvalidSlug,
    #[error("plan artifact already finalized")]
    AlreadyFinalized,
    #[error("plan artifact not finalized")]
    #[allow(dead_code)]
    NotFinalized,
}

impl PlanArtifact {
    pub fn new_temp(
        ody_home: AbsolutePathBuf,
        thread_id: ody_protocol::ThreadId,
        date: &str,
    ) -> Self {
        let temp_path = allocate_temp_path(&ody_home, &thread_id, date);
        Self {
            state: Mutex::new(PlanArtifactState::Temporary { temp_path }),
            ody_home,
            thread_id,
            date: date.to_string(),
        }
    }

    pub fn path(&self) -> Option<PathBuf> {
        let guard = self.state.try_lock().ok()?;
        match &*guard {
            PlanArtifactState::Temporary { temp_path } => Some(temp_path.clone()),
            PlanArtifactState::Finalized { final_path } => Some(final_path.clone()),
            PlanArtifactState::InlineOnly => None,
        }
    }

    pub async fn finalize_name(&self, slug: &str) -> Result<(), PlanArtifactError> {
        let mut state = self.state.lock().await;
        match &*state {
            PlanArtifactState::Finalized { .. } => Err(PlanArtifactError::AlreadyFinalized),
            PlanArtifactState::InlineOnly => Ok(()),
            PlanArtifactState::Temporary { .. } => {
                let sanitized = sanitize_plan_slug(slug);
                let final_name = format!("{}-{}.md", self.date, sanitized);
                let final_path = self.ody_home.as_path().join("plans").join(final_name);
                *state = PlanArtifactState::Finalized { final_path };
                Ok(())
            }
        }
    }

    pub async fn write_plan(&self, _markdown: &str, persist: bool) -> PlanWriteOutcome {
        let mut state = self.state.lock().await;
        if !persist {
            *state = PlanArtifactState::InlineOnly;
            return PlanWriteOutcome::InlineOnly;
        }

        let path = match &*state {
            PlanArtifactState::Temporary { temp_path } => temp_path.clone(),
            PlanArtifactState::Finalized { final_path } => final_path.clone(),
            PlanArtifactState::InlineOnly => {
                return PlanWriteOutcome::InlineOnly;
            }
        };

        // PM 4.0 stub: do not perform actual disk IO.
        PlanWriteOutcome::Written { path }
    }

    pub fn is_plan_file_path(&self, target: &Path) -> bool {
        let Some(plan_path) = self.path() else {
            return false;
        };
        ody_utils_path::paths_match_after_normalization(&plan_path, target)
    }

    pub fn restore_or_create(
        ody_home: AbsolutePathBuf,
        thread_id: ody_protocol::ThreadId,
        stored_path: Option<PathBuf>,
        date: &str,
    ) -> Self {
        match stored_path {
            Some(path) if path.exists() => Self {
                state: Mutex::new(PlanArtifactState::Finalized { final_path: path }),
                ody_home,
                thread_id,
                date: date.to_string(),
            },
            _ => Self::new_temp(ody_home, thread_id, date),
        }
    }
}

fn allocate_temp_path(
    ody_home: &AbsolutePathBuf,
    thread_id: &ody_protocol::ThreadId,
    date: &str,
) -> PathBuf {
    let plans_dir = ody_home.as_path().join("plans");
    let filename = format!("tmp-{thread_id}-{date}.md");
    plans_dir.join(filename)
}

/// Sanitize a user prompt into a plan file slug.
pub(crate) fn sanitize_plan_slug(prompt: &str) -> String {
    let slug = sanitize_name(prompt);
    if slug.is_empty() || slug == "app" {
        "plan".to_string()
    } else {
        slug
    }
}

// Reuse the existing slug sanitizer used by plugins.
fn sanitize_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            normalized.push(character.to_ascii_lowercase());
        } else {
            normalized.push('-');
        }
    }
    let normalized = normalized.trim_matches('-');
    if normalized.is_empty() {
        "app".to_string()
    } else {
        normalized
            .split('-')
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>()
            .join("_")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_utils_absolute_path::AbsolutePathBuf;

    fn test_artifact(date: &str) -> (PlanArtifact, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let ody_home = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let thread_id =
            ody_protocol::ThreadId::from_string("00000000-0000-0000-0000-000000000001").unwrap();
        (PlanArtifact::new_temp(ody_home, thread_id, date), tmp)
    }

    #[test]
    fn new_temp_allocates_under_ody_home_plans() {
        let (artifact, tmp) = test_artifact("2026-07-04");
        let path = artifact.path().unwrap();
        assert!(path.starts_with(tmp.path().join("plans")));
        assert!(path
            .to_string_lossy()
            .contains("tmp-00000000-0000-0000-0000-000000000001-2026-07-04.md"));
    }

    #[test]
    fn sanitize_plan_slug_ascii() {
        assert_eq!(
            sanitize_plan_slug("Refactor the auth module!"),
            "refactor_the_auth_module"
        );
    }

    #[test]
    fn sanitize_plan_slug_collapses_whitespace_and_punctuation() {
        assert_eq!(
            sanitize_plan_slug("  Fix   the   bug  in parser!!!  "),
            "fix_the_bug_in_parser"
        );
    }

    #[test]
    fn sanitize_plan_slug_non_ascii_fallback() {
        assert_eq!(sanitize_plan_slug("你好世界"), "plan");
    }

    #[test]
    fn sanitize_plan_slug_empty_fallback() {
        assert_eq!(sanitize_plan_slug("!!!"), "plan");
    }

    #[tokio::test]
    async fn finalize_name_moves_state_to_finalized() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        artifact.finalize_name("refactor_auth").await.unwrap();
        let path = artifact.path().unwrap();
        assert!(path.to_string_lossy().ends_with("2026-07-04-refactor_auth.md"));
    }

    #[tokio::test]
    async fn finalize_name_twice_returns_already_finalized() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        artifact.finalize_name("refactor_auth").await.unwrap();
        let err = artifact.finalize_name("other").await.unwrap_err();
        assert!(matches!(err, PlanArtifactError::AlreadyFinalized));
    }

    #[tokio::test]
    async fn write_plan_with_persist_true_returns_written_path() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        artifact.finalize_name("refactor_auth").await.unwrap();
        let outcome = artifact.write_plan("# Plan\n", true).await;
        assert!(matches!(outcome, PlanWriteOutcome::Written { path } if path.to_string_lossy().ends_with("2026-07-04-refactor_auth.md")));
    }

    #[tokio::test]
    async fn write_plan_with_persist_false_returns_inline_only() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        let outcome = artifact.write_plan("# Plan\n", false).await;
        assert!(matches!(outcome, PlanWriteOutcome::InlineOnly));
        assert!(matches!(&*artifact.state.lock().await, PlanArtifactState::InlineOnly));
    }

    #[tokio::test]
    async fn is_plan_file_path_matches_own_path() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        let path = artifact.path().unwrap();
        assert!(artifact.is_plan_file_path(&path));
    }

    #[tokio::test]
    async fn is_plan_file_path_rejects_relative_bypass() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        let path = artifact.path().unwrap();
        let relative = path.parent().unwrap().join("../other.md");
        assert!(!artifact.is_plan_file_path(&relative));
    }

    #[tokio::test]
    async fn restore_or_create_uses_existing_path() {
        let tmp = tempfile::tempdir().unwrap();
        let ody_home = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let existing = tmp.path().join("plans").join("2026-07-04-existing.md");
        std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
        std::fs::write(&existing, "# Existing").unwrap();
        let thread_id =
            ody_protocol::ThreadId::from_string("00000000-0000-0000-0000-000000000002").unwrap();
        let artifact =
            PlanArtifact::restore_or_create(ody_home, thread_id, Some(existing.clone()), "2026-07-04");
        assert!(artifact.is_plan_file_path(&existing));
    }
}
