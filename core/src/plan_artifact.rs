use ody_config::config_toml::PlanModeTier;
use ody_utils_absolute_path::AbsolutePathBuf;
use ody_utils_path::write_atomically;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex;

/// Session-level plan/design file artifact. Lives in Plan or Design mode.
///
/// `plans_base_dir` is the directory that contains the `plans/` or `designs/`
/// sub-directory. Historically this was `ODY_HOME`; it is now the current
/// project directory's `.ody-code` folder (e.g. `{cwd}/.ody-code`).
#[derive(Debug)]
pub struct PlanArtifact {
    state: Mutex<PlanArtifactState>,
    submitted: AtomicBool,
    last_manifest_snapshot: Mutex<Option<ManifestSnapshot>>,
    last_plan_text: Mutex<Option<String>>,
    /// 1-based count of plan-mode after-turn calls for this artifact.
    plan_mode_turn_count: StdMutex<usize>,
    /// Turn at which the last full reminder was injected; `Some(0)` means "before turn 1".
    last_full_turn: StdMutex<Option<usize>>,
    /// Turn at which any reminder was last injected; `Some(0)` means "before turn 1".
    last_any_turn: StdMutex<Option<usize>>,
    current_tier: StdMutex<Option<PlanModeTier>>,
    plans_base_dir: AbsolutePathBuf,
    /// Sub-directory under `plans_base_dir` that holds this artifact's files
    /// (e.g. `"plans"` for Plan mode, `"designs"` for Design mode).
    subdir: &'static str,
    thread_id: ody_protocol::ThreadId,
    date: String,
}

#[derive(Debug, PartialEq)]
pub enum PlanArtifactState {
    Temporary { temp_path: PathBuf },
    Finalized { final_path: PathBuf },
    InlineOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManifestSnapshot {
    pub done_count: usize,
    pub pending_count: usize,
    pub rows: Vec<PartRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartRow {
    pub file_name: String,
    pub scope: String,
    pub status: PartStatus,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PartStatus {
    Pending,
    Done,
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
    /// Plan-mode artifact rooted under `<base>/plans/`.
    #[allow(dead_code)] // symmetric counterpart to `new_design`; reserved for D4+ call sites.
    pub fn new_plan(
        plans_base_dir: AbsolutePathBuf,
        thread_id: ody_protocol::ThreadId,
        date: &str,
    ) -> Self {
        Self::new_temp(plans_base_dir, thread_id, date)
    }

    /// Design-mode artifact rooted under `<base>/designs/`.
    #[allow(dead_code)] // symmetric counterpart to `new_plan`; reserved for D3/D4 call sites.
    pub fn new_design(
        plans_base_dir: AbsolutePathBuf,
        thread_id: ody_protocol::ThreadId,
        date: &str,
    ) -> Self {
        Self::with_subdir(plans_base_dir, "designs", thread_id, date)
    }

    fn with_subdir(
        plans_base_dir: AbsolutePathBuf,
        subdir: &'static str,
        thread_id: ody_protocol::ThreadId,
        date: &str,
    ) -> Self {
        let temp_path = allocate_temp_path(&plans_base_dir, subdir, &thread_id, date);
        Self {
            state: Mutex::new(PlanArtifactState::Temporary { temp_path }),
            submitted: AtomicBool::new(false),
            last_manifest_snapshot: Mutex::new(None),
            last_plan_text: Mutex::new(None),
            plan_mode_turn_count: StdMutex::new(0),
            last_full_turn: StdMutex::new(Some(0)),
            last_any_turn: StdMutex::new(Some(0)),
            current_tier: StdMutex::new(None),
            plans_base_dir,
            subdir,
            thread_id,
            date: date.to_string(),
        }
    }


    /// Mark the plan as submitted. Plan-mode turn loop will end the turn on
    /// the next `take_submitted` check.
    pub fn mark_submitted(&self) {
        self.submitted.store(true, Ordering::Release);
    }

    /// Take the submitted flag. Returns true exactly once after
    /// `mark_submitted` was called, resetting it to false.
    pub fn take_submitted(&self) -> bool {
        self.submitted.swap(false, Ordering::AcqRel)
    }

    pub fn new_temp(
        plans_base_dir: AbsolutePathBuf,
        thread_id: ody_protocol::ThreadId,
        date: &str,
    ) -> Self {
        Self::with_subdir(plans_base_dir, "plans", thread_id, date)
    }

    pub fn path(&self) -> Option<PathBuf> {
        let guard = self.state.try_lock().ok()?;
        match &*guard {
            PlanArtifactState::Temporary { temp_path } => Some(temp_path.clone()),
            PlanArtifactState::Finalized { final_path } => Some(final_path.clone()),
            PlanArtifactState::InlineOnly => None,
        }
    }

    /// Explicit override for the title-based auto-finalization `write_plan` does
    /// on its first persisted write. No production call site left after that
    /// change (finalization is now driven by the plan's own title); kept as a
    /// public escape hatch and heavily used by tests to pre-finalize an artifact
    /// to a known path.
    #[allow(dead_code)]
    pub async fn finalize_name(&self, slug: &str) -> Result<(), PlanArtifactError> {
        let mut state = self.state.lock().await;
        match &*state {
            PlanArtifactState::Finalized { .. } => Err(PlanArtifactError::AlreadyFinalized),
            PlanArtifactState::InlineOnly => Ok(()),
            PlanArtifactState::Temporary { .. } => {
                self.apply_finalized_name(&mut state, slug);
                Ok(())
            }
        }
    }

    /// Rewrites `state` in place to `Finalized` using `slug`. No-op unless
    /// `state` is still `Temporary`. Caller must already hold the lock.
    fn apply_finalized_name(&self, state: &mut PlanArtifactState, slug: &str) {
        if matches!(state, PlanArtifactState::Temporary { .. }) {
            let sanitized = sanitize_plan_slug(slug);
            let final_name = format!("{}-{}.md", self.date, sanitized);
            let final_path = self.plans_base_dir.as_path().join(self.subdir).join(final_name);
            *state = PlanArtifactState::Finalized { final_path };
        }
    }

    pub async fn write_plan(&self, markdown: &str, persist: bool) -> PlanWriteOutcome {
        if let Ok(mut guard) = self.last_plan_text.try_lock() {
            *guard = Some(markdown.to_string());
        }
        let mut state = self.state.lock().await;
        if !persist {
            *state = PlanArtifactState::InlineOnly;
            return PlanWriteOutcome::InlineOnly;
        }

        // First real write: name the file from the plan's own title instead of
        // leaving it under the `tmp-<thread_id>-<date>.md` placeholder. A later
        // write (e.g. a split plan's part turns) sees `Finalized` already and
        // `apply_finalized_name` is a no-op, so the path stays stable.
        let slug = slug_from_markdown_title(markdown).unwrap_or_else(|| "plan".to_string());
        self.apply_finalized_name(&mut state, &slug);

        let path = match &*state {
            PlanArtifactState::Temporary { temp_path } => temp_path.clone(),
            PlanArtifactState::Finalized { final_path } => final_path.clone(),
            PlanArtifactState::InlineOnly => {
                return PlanWriteOutcome::InlineOnly;
            }
        };

        match write_atomically(&path, markdown) {
            Ok(()) => PlanWriteOutcome::Written { path },
            Err(error) => {
                *state = PlanArtifactState::InlineOnly;
                PlanWriteOutcome::Failed {
                    error: PlanArtifactError::Io(error),
                }
            }
        }
    }

    pub fn last_manifest_snapshot(&self) -> Option<ManifestSnapshot> {
        self.last_manifest_snapshot.try_lock().ok()?.clone()
    }

    pub fn set_last_manifest_snapshot(&self, snapshot: ManifestSnapshot) {
        if let Ok(mut guard) = self.last_manifest_snapshot.try_lock() {
            *guard = Some(snapshot);
        }
    }

    pub fn clear_last_manifest_snapshot(&self) {
        if let Ok(mut guard) = self.last_manifest_snapshot.try_lock() {
            *guard = None;
        }
    }

    /// Returns the next 1-based plan-mode turn number and advances the counter.
    pub fn next_plan_mode_turn(&self) -> usize {
        let mut guard = self
            .plan_mode_turn_count
            .lock()
            .expect("plan_mode_turn_count poisoned");
        *guard += 1;
        *guard
    }

    /// Returns `(last_full_turn, last_any_turn)` for reminder-cadence selection.
    pub fn last_reminder_turns(&self) -> (Option<usize>, Option<usize>) {
        let full = *self.last_full_turn.lock().expect("last_full_turn poisoned");
        let any = *self.last_any_turn.lock().expect("last_any_turn poisoned");
        (full, any)
    }

    /// Records that a reminder was injected at `turn`. `full` is `true` for a full reminder.
    pub fn record_reminder_injected(&self, full: bool, turn: usize) {
        if full {
            *self.last_full_turn.lock().expect("last_full_turn poisoned") = Some(turn);
        }
        *self.last_any_turn.lock().expect("last_any_turn poisoned") = Some(turn);
    }

    /// Returns the tier resolved at session start, if any.
    pub fn plan_mode_tier(&self) -> Option<PlanModeTier> {
        *self.current_tier.lock().expect("current_tier poisoned")
    }

    /// Records the resolved tier.
    pub fn set_plan_mode_tier(&self, tier: PlanModeTier) {
        *self.current_tier.lock().expect("current_tier poisoned") = Some(tier);
    }

    pub fn last_plan_text(&self) -> Option<String> {
        self.last_plan_text.try_lock().ok()?.clone()
    }

    pub fn stem_dir(&self) -> Option<PathBuf> {
        let path = self.path()?;
        let stem = path.file_stem()?;
        Some(path.with_file_name(stem))
    }

    /// Returns true if `target` is this artifact's main file or a part file
    /// under its stem sub-directory. Applies to both plan and design artifacts.
    pub fn is_plan_file_path(&self, target: &Path) -> bool {
        let Some(plan_path) = self.path() else {
            return false;
        };

        if ody_utils_path::paths_match_after_normalization(&plan_path, target) {
            return true;
        }

        let Some(stem_dir) = plan_path
            .file_stem()
            .map(|stem| plan_path.with_file_name(stem))
        else {
            return false;
        };

        let Some(target_parent) = target.parent() else {
            return false;
        };

        if !target.extension().map_or(false, |ext| ext == "md") {
            return false;
        }

        ody_utils_path::paths_match_after_normalization(&stem_dir, target_parent)
    }

    pub fn restore_or_create(
        plans_base_dir: AbsolutePathBuf,
        thread_id: ody_protocol::ThreadId,
        stored_path: Option<PathBuf>,
        date: &str,
    ) -> Self {
        match stored_path {
            Some(path) if path.exists() => {
                let subdir = infer_subdir(&plans_base_dir, &path);
                Self {
                    state: Mutex::new(PlanArtifactState::Finalized { final_path: path }),
                    submitted: AtomicBool::new(false),
                    last_manifest_snapshot: Mutex::new(None),
                    last_plan_text: Mutex::new(None),
                    plan_mode_turn_count: StdMutex::new(0),
                    last_full_turn: StdMutex::new(Some(0)),
                    last_any_turn: StdMutex::new(Some(0)),
                    current_tier: StdMutex::new(None),
                    plans_base_dir,
                    subdir,
                    thread_id,
                    date: date.to_string(),
                }
            }
            _ => Self::new_temp(plans_base_dir, thread_id, date),
        }
    }
}

fn allocate_temp_path(
    plans_base_dir: &AbsolutePathBuf,
    subdir: &str,
    thread_id: &ody_protocol::ThreadId,
    date: &str,
) -> PathBuf {
    let plans_dir = plans_base_dir.as_path().join(subdir);
    let filename = format!("tmp-{thread_id}-{date}.md");
    plans_dir.join(filename)
}

fn infer_subdir(plans_base_dir: &AbsolutePathBuf, path: &Path) -> &'static str {
    let base = plans_base_dir.as_path();
    if path.starts_with(base.join("designs")) {
        "designs"
    } else {
        "plans"
    }
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

/// Upper bound on a sanitized slug's length. Without this, slugifying an
/// unbounded input (e.g. a whole user prompt embedding full file paths)
/// produces filenames hundreds of characters long.
const MAX_SLUG_LEN: usize = 60;

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
        return "app".to_string();
    }
    let joined = normalized
        .split('-')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    truncate_slug(&joined, MAX_SLUG_LEN)
}

/// Truncates a `_`-joined slug to at most `max_len` bytes without cutting a
/// word in half. The sanitized alphabet is ASCII-only, so byte indices are
/// always char boundaries.
fn truncate_slug(slug: &str, max_len: usize) -> String {
    if slug.len() <= max_len {
        return slug.to_string();
    }
    let truncated = &slug[..max_len];
    match truncated.rfind('_') {
        Some(idx) if idx > 0 => truncated[..idx].to_string(),
        _ => truncated.trim_end_matches('_').to_string(),
    }
}

/// Extracts a short slug from the plan/design markdown's own title line
/// (`# <Feature> Implementation Plan`, mandated by the plan/design prompt
/// contracts as the first line of every plan). This lets the model name its
/// own plan file — mirroring ody-code's "invent your own filename" contract
/// — instead of mechanically slugifying the entire raw user prompt, which has
/// no natural length bound and can embed whole file paths verbatim.
fn slug_from_markdown_title(markdown: &str) -> Option<String> {
    let title_line = markdown
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with('#'))?;
    let title = title_line.trim_start_matches('#').trim();
    if title.is_empty() {
        return None;
    }
    match sanitize_name(title).as_str() {
        "" | "app" => None,
        _ => Some(sanitize_name(title)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_utils_absolute_path::AbsolutePathBuf;

    fn test_artifact(date: &str) -> (PlanArtifact, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let thread_id =
            ody_protocol::ThreadId::from_string("00000000-0000-0000-0000-000000000001").unwrap();
        (PlanArtifact::new_temp(plans_base_dir, thread_id, date), tmp)
    }

    #[test]
    fn new_temp_allocates_under_plans_base_dir_plans() {
        let (artifact, tmp) = test_artifact("2026-07-04");
        let path = artifact.path().unwrap();
        assert!(path.starts_with(tmp.path().join("plans")));
        assert!(path
            .to_string_lossy()
            .contains("tmp-00000000-0000-0000-0000-000000000001-2026-07-04.md"));
    }


    #[test]
    fn submitted_flag_defaults_false_and_toggles_once() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        assert!(!artifact.take_submitted());
        artifact.mark_submitted();
        assert!(artifact.take_submitted());
        assert!(!artifact.take_submitted());
    }
    #[tokio::test]
    async fn new_design_allocates_under_designs() {
        let tmp = tempfile::tempdir().unwrap();
        let base = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let thread_id =
            ody_protocol::ThreadId::from_string("00000000-0000-0000-0000-000000000001").unwrap();
        let artifact = PlanArtifact::new_design(base, thread_id, "2026-07-04");

        let temp_path = artifact.path().unwrap();
        assert!(temp_path.starts_with(tmp.path().join("designs")));

        artifact.finalize_name("auth_flow").await.unwrap();
        let final_path = artifact.path().unwrap();
        assert!(final_path.starts_with(tmp.path().join("designs")));
        assert!(final_path
            .to_string_lossy()
            .ends_with("2026-07-04-auth_flow.md"));

        assert!(artifact.is_plan_file_path(&final_path));
        let stem_dir = final_path.with_extension("");
        assert!(artifact.is_plan_file_path(&stem_dir.join("core.md")));
    }

    #[test]
    fn infer_subdir_designs() {
        let tmp = tempfile::tempdir().unwrap();
        let base = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let design_path = tmp.path().join("designs").join("2026-07-04-auth.md");
        assert_eq!(infer_subdir(&base, &design_path), "designs");
    }

    #[test]
    fn infer_subdir_plans() {
        let tmp = tempfile::tempdir().unwrap();
        let base = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let plan_path = tmp.path().join("plans").join("2026-07-04-auth.md");
        assert_eq!(infer_subdir(&base, &plan_path), "plans");
    }

    #[tokio::test]
    async fn restore_or_create_uses_design_path() {
        let tmp = tempfile::tempdir().unwrap();
        let base = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let existing = tmp.path().join("designs").join("2026-07-04-existing.md");
        std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
        std::fs::write(&existing, "# Existing").unwrap();
        let thread_id =
            ody_protocol::ThreadId::from_string("00000000-0000-0000-0000-000000000002").unwrap();
        let artifact =
            PlanArtifact::restore_or_create(base, thread_id, Some(existing.clone()), "2026-07-04");
        assert!(artifact.is_plan_file_path(&existing));
        let path = artifact.path().unwrap();
        assert!(path.starts_with(tmp.path().join("designs")));
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
    fn sanitize_plan_slug_truncates_long_input() {
        // Regression: naming used to slugify the *entire* raw user prompt with no
        // length cap, so a /writing-plan prompt embedding two absolute file paths
        // produced 150+ char filenames. `sanitize_name` must cap output length
        // regardless of caller.
        let long_prompt = "请阅读文件 .ody-code/designs/2026-07-10-d6-design-plan-handoff.md 的内容，并将其转换为一份完整、可执行的执行计划，写入计划文件 /Users/ranwei/workspace/rust_work/ody-rs/.ody-code/plans/2026-07-10-d6-design-plan-handoff.md。";
        let slug = sanitize_plan_slug(long_prompt);
        assert!(
            slug.len() <= 60,
            "slug should be capped at 60 chars, got {} chars: {slug}",
            slug.len()
        );
        assert!(
            !slug.ends_with('_'),
            "truncation should not leave a trailing separator: {slug}"
        );
    }

    #[tokio::test]
    async fn write_plan_finalizes_name_from_markdown_title_not_raw_prompt() {
        // Mirrors ody-code's "invent your own filename" contract: the plan's own
        // title (which the rigor-tier header mandates as `# <Feature> Implementation
        // Plan`) names the file, not a mechanical slugify of the user's raw prompt.
        let (artifact, tmp) = test_artifact("2026-07-10");
        let markdown = "# D6 Design to Plan Handoff Implementation Plan\n\n**Goal:** ...";
        let outcome = artifact.write_plan(markdown, true).await;
        let path = match outcome {
            PlanWriteOutcome::Written { path } => path,
            other => panic!("expected Written, got {other:?}"),
        };
        assert!(path.starts_with(tmp.path().join("plans")));
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            "2026-07-10-d6_design_to_plan_handoff_implementation_plan.md"
        );
    }

    #[tokio::test]
    async fn write_plan_falls_back_to_plan_slug_when_markdown_has_no_title() {
        let (artifact, _tmp) = test_artifact("2026-07-10");
        let outcome = artifact.write_plan("no heading here, just prose", true).await;
        let path = match outcome {
            PlanWriteOutcome::Written { path } => path,
            other => panic!("expected Written, got {other:?}"),
        };
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            "2026-07-10-plan.md"
        );
    }

    #[tokio::test]
    async fn write_plan_does_not_refinalize_an_already_finalized_artifact() {
        let (artifact, _tmp) = test_artifact("2026-07-10");
        artifact.finalize_name("explicit_name").await.unwrap();
        let outcome = artifact
            .write_plan("# A Totally Different Title\n\nbody", true)
            .await;
        let path = match outcome {
            PlanWriteOutcome::Written { path } => path,
            other => panic!("expected Written, got {other:?}"),
        };
        // Already-finalized paths (e.g. split-plan part turns re-writing the index)
        // must stay stable — the title on a later write must not rename the file.
        assert!(path.to_string_lossy().ends_with("explicit_name.md"));
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
        let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let existing = tmp.path().join("plans").join("2026-07-04-existing.md");
        std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
        std::fs::write(&existing, "# Existing").unwrap();
        let thread_id =
            ody_protocol::ThreadId::from_string("00000000-0000-0000-0000-000000000002").unwrap();
        let artifact =
            PlanArtifact::restore_or_create(plans_base_dir, thread_id, Some(existing.clone()), "2026-07-04");
        assert!(artifact.is_plan_file_path(&existing));
    }

    #[tokio::test]
    async fn write_plan_persists_markdown_to_disk() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        artifact.finalize_name("refactor_auth").await.unwrap();
        let markdown = "# Plan\n\n- Step 1\n";
        let outcome = artifact.write_plan(markdown, true).await;
        let path = match outcome {
            PlanWriteOutcome::Written { path } => path,
            other => panic!("expected Written, got {other:?}"),
        };
        assert_eq!(std::fs::read_to_string(&path).unwrap(), markdown);
    }

    #[tokio::test]
    async fn write_plan_creates_parent_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let thread_id =
            ody_protocol::ThreadId::from_string("00000000-0000-0000-0000-000000000001").unwrap();
        let artifact = PlanArtifact::new_temp(plans_base_dir, thread_id, "2026-07-04");
        artifact.finalize_name("refactor_auth").await.unwrap();

        let outcome = artifact.write_plan("# Plan\n", true).await;
        let path = match outcome {
            PlanWriteOutcome::Written { path } => path,
            other => panic!("expected Written, got {other:?}"),
        };

        assert!(path.parent().unwrap().exists());
    }

    #[tokio::test]
    async fn write_plan_failure_falls_back_to_inline_only() {
        let tmp = tempfile::tempdir().unwrap();
        // Make plans_base_dir a file so create_dir_all(plans) fails.
        let plans_base_dir_file = tmp.path().join("plans_base_dir_file");
        std::fs::write(&plans_base_dir_file, "").unwrap();
        let plans_base_dir = AbsolutePathBuf::from_absolute_path(&plans_base_dir_file).unwrap();
        let thread_id =
            ody_protocol::ThreadId::from_string("00000000-0000-0000-0000-000000000001").unwrap();
        let artifact = PlanArtifact::new_temp(plans_base_dir, thread_id, "2026-07-04");

        let outcome = artifact.write_plan("# Plan\n", true).await;

        assert!(
            matches!(outcome, PlanWriteOutcome::Failed { .. }),
            "expected Failed when parent cannot be created, got {outcome:?}"
        );
        assert!(matches!(
            &*artifact.state.lock().await,
            PlanArtifactState::InlineOnly
        ));
    }

    #[tokio::test]
    async fn is_plan_file_path_matches_stem_subdirectory_md() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        artifact.finalize_name("refactor_auth").await.unwrap();
        let plan_path = artifact.path().unwrap();
        let stem_dir = plan_path.with_extension("");
        let sub_file = stem_dir.join("subsystem.md");
        assert!(
            artifact.is_plan_file_path(&sub_file),
            "expected {sub_file:?} to be whitelisted under {stem_dir:?}"
        );
    }

    #[tokio::test]
    async fn is_plan_file_path_rejects_non_md_in_stem_subdirectory() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        artifact.finalize_name("refactor_auth").await.unwrap();
        let plan_path = artifact.path().unwrap();
        let stem_dir = plan_path.with_extension("");
        let sub_file = stem_dir.join("subsystem.txt");
        assert!(!artifact.is_plan_file_path(&sub_file));
    }

    #[tokio::test]
    async fn is_plan_file_path_rejects_sibling_stem_subdirectory() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        artifact.finalize_name("refactor_auth").await.unwrap();
        let plan_path = artifact.path().unwrap();
        let sibling_dir = plan_path.parent().unwrap().join("2026-07-04-other");
        let sub_file = sibling_dir.join("subsystem.md");
        assert!(!artifact.is_plan_file_path(&sub_file));
    }

    #[tokio::test]
    async fn manifest_snapshot_round_trip() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        assert!(artifact.last_manifest_snapshot().is_none());

        let snapshot = ManifestSnapshot {
            done_count: 1,
            pending_count: 2,
            rows: vec![
                PartRow {
                    file_name: "core.md".to_string(),
                    scope: "models".to_string(),
                    status: PartStatus::Done,
                },
                PartRow {
                    file_name: "api.md".to_string(),
                    scope: "endpoints".to_string(),
                    status: PartStatus::Pending,
                },
            ],
        };
        artifact.set_last_manifest_snapshot(snapshot.clone());
        assert_eq!(artifact.last_manifest_snapshot(), Some(snapshot));

        artifact.clear_last_manifest_snapshot();
        assert!(artifact.last_manifest_snapshot().is_none());
    }

    #[tokio::test]
    async fn stem_dir_returns_plan_stem_directory() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        artifact.finalize_name("refactor_auth").await.unwrap();
        let stem = artifact.stem_dir().unwrap();
        assert!(stem.to_string_lossy().ends_with("2026-07-04-refactor_auth"));
    }

    #[tokio::test]
    async fn is_plan_file_path_allows_part_file_under_stem() {
        let (artifact, _tmp) = test_artifact("2026-07-04");
        artifact.finalize_name("topic").await.unwrap();
        let plan_path = artifact.path().unwrap();
        let stem_dir = plan_path.with_extension("");
        let part_file = stem_dir.join("core.md");
        assert!(artifact.is_plan_file_path(&part_file));
    }

#[test]
fn default_plan_mode_tier_is_none() {
    let (artifact, _tmp) = test_artifact("2026-07-04");
    assert_eq!(artifact.plan_mode_tier(), None);
}

#[test]
fn plan_mode_tier_round_trip() {
    let (artifact, _tmp) = test_artifact("2026-07-04");
    artifact.set_plan_mode_tier(PlanModeTier::Rigor);
    assert_eq!(artifact.plan_mode_tier(), Some(PlanModeTier::Rigor));

    artifact.set_plan_mode_tier(PlanModeTier::Concise);
    assert_eq!(artifact.plan_mode_tier(), Some(PlanModeTier::Concise));
}

}
