//! Durable manual acceptance runs served by a capability-protected loopback site.
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, bail};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub action: String,
    pub expected: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub title: String,
    pub reason: String,
    pub prerequisites: String,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub title: String,
    pub cases: Vec<Case>,
    pub previous_run: Option<Uuid>,
}

impl Plan {
    pub fn validate(&self) -> Result<()> {
        if self.title.trim().is_empty() || self.cases.is_empty() || self.cases.len() > 100 {
            bail!("title and 1..100 cases are required");
        }
        let mut ids = std::collections::HashSet::new();
        for case in &self.cases {
            if case.id.is_empty()
                || case.id.len() > 32
                || !case
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || !ids.insert(&case.id)
                || case.title.trim().is_empty()
                || case.reason.trim().is_empty()
                || case.steps.is_empty()
                || case.steps.len() > 100
                || case
                    .steps
                    .iter()
                    .any(|s| s.action.trim().is_empty() || s.expected.trim().is_empty())
            {
                bail!(
                    "each case needs a unique 1..32 ASCII alphanumeric/_/- ID, title, manual reason, and 1..100 action/expected steps"
                );
            }
        }
        if serde_json::to_vec(self)?.len() > 256 * 1024 {
            bail!("acceptance plan exceeds 256 KiB");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    Blocked,
    NotRun,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepFeedback {
    /// One-based index in the immutable case plan.
    pub step_index: usize,
    pub outcome: Outcome,
    pub actual: String,
    pub evidence: String,
}

pub fn aggregate_steps(steps: &[StepFeedback]) -> Outcome {
    if steps.iter().any(|s| s.outcome == Outcome::Failed) {
        Outcome::Failed
    } else if steps.iter().any(|s| s.outcome == Outcome::Blocked) {
        Outcome::Blocked
    } else if steps.is_empty() || steps.iter().any(|s| s.outcome == Outcome::NotRun) {
        Outcome::NotRun
    } else {
        Outcome::Passed
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feedback {
    pub case_id: String,
    pub outcome: Outcome,
    pub actual: String,
    /// Human-entered evidence references; never opened or executed by the server.
    pub evidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<StepFeedback>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    #[serde(default = "legacy_feedback_version")]
    pub feedback_version: u8,
    pub id: Uuid,
    pub thread_id: String,
    pub plan: Plan,
    pub feedback: Vec<Feedback>,
    pub submitted: bool,
    pub revision: u64,
    #[serde(default)]
    pub notification: NotificationStatus,
}

fn legacy_feedback_version() -> u8 {
    1
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationStatus {
    #[default]
    NotConfigured,
    Pending,
    Queued,
}

/// The host enqueues this signal; free-form feedback is never an instruction.
#[derive(Clone)]
pub struct SubmissionNotice {
    pub run_id: Uuid,
    pub thread_id: String,
}

pub type SubmissionNotifier =
    Arc<dyn Fn(SubmissionNotice) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    revision: u64,
    feedback: Vec<Feedback>,
    submit: bool,
}

impl Run {
    fn update(&mut self, mut update: Update) -> Result<()> {
        if self.submitted {
            bail!("submitted runs are immutable; create a linked retest");
        }
        if update.revision != self.revision {
            bail!("stale revision; reload before saving");
        }
        let mut ids = std::collections::HashSet::new();
        for item in &mut update.feedback {
            if !ids.insert(&item.case_id) || !self.plan.cases.iter().any(|c| c.id == item.case_id) {
                bail!("unknown or duplicate case ID");
            }
            let case = self
                .plan
                .cases
                .iter()
                .find(|c| c.id == item.case_id)
                .ok_or_else(|| anyhow::anyhow!("unknown case ID"))?;
            if let Some(steps) = &item.steps {
                if steps.len() != case.steps.len()
                    || steps.iter().enumerate().any(|(i, s)| s.step_index != i + 1)
                {
                    bail!("step feedback must cover every plan step exactly once in order");
                }
                for step in steps {
                    if matches!(step.outcome, Outcome::Failed | Outcome::Blocked)
                        && step.actual.trim().is_empty()
                    {
                        bail!("failed/blocked steps require an explanation");
                    }
                    if step.actual.len() + step.evidence.len() > 8 * 1024 {
                        bail!("step feedback too large");
                    }
                }
                item.outcome = aggregate_steps(steps);
                if update.submit && item.outcome == Outcome::NotRun {
                    bail!("unfinished cases cannot be submitted without a failed/blocked step");
                }
            } else {
                if self.feedback_version >= 2 || update.submit {
                    bail!("step feedback required; legacy observations are not step results");
                }
                if matches!(item.outcome, Outcome::Failed | Outcome::Blocked)
                    && item.actual.trim().is_empty()
                {
                    bail!("failed/blocked cases require an explanation");
                }
            }
            if item.actual.len() + item.evidence.len() > 8 * 1024 {
                bail!("feedback too large");
            }
        }
        if update.submit && update.feedback.len() != self.plan.cases.len() {
            bail!("every case must have an outcome before submission");
        }
        self.feedback = update.feedback;
        self.submitted = update.submit;
        self.revision += 1;
        Ok(())
    }
}


/// Discover the twenty most recently modified pending runs, without plans/feedback.
pub async fn pending_runs(root: &Path, thread: &str) -> Result<Vec<Uuid>> {
    reject_symlink(root).await?;
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut pending = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| Uuid::parse_str(s).ok())
        else {
            continue;
        };
        // Invalid data must not silently result in duplicate acceptance creation.
        let run = load(root, id, thread).await?;
        if !run.submitted {
            pending.push((entry.metadata().await?.modified()?, id));
            pending.sort_by(|a, b| b.cmp(a));
            pending.truncate(20);
        }
    }
    Ok(pending.into_iter().map(|(_, id)| id).collect())
}

pub async fn load(root: &Path, id: Uuid, thread: &str) -> Result<Run> {
    let path = root.join(format!("{id}.json"));
    reject_symlink(&path).await?;
    let run: Run = serde_json::from_slice(&tokio::fs::read(path).await?)?;
    if run.id != id || run.thread_id != thread {
        bail!("run does not belong to this thread");
    }
    Ok(run)
}

async fn persist(root: &Path, run: &Run) -> Result<()> {
    reject_symlink(root).await?;
    tokio::fs::create_dir_all(root).await?;
    let temporary = root.join(format!(".{}.tmp", Uuid::new_v4()));
    tokio::fs::write(&temporary, serde_json::to_vec_pretty(run)?).await?;
    if let Err(error) = tokio::fs::rename(&temporary, root.join(format!("{}.json", run.id))).await {
        let _ = tokio::fs::remove_file(temporary).await;
        return Err(error.into());
    }
    Ok(())
}

async fn reject_symlink(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match tokio::fs::symlink_metadata(ancestor).await {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("symlinked acceptance paths are not allowed")
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub async fn create(root: &Path, thread: String, plan: Plan) -> Result<Run> {
    plan.validate()?;
    if let Some(previous) = plan.previous_run {
        load(root, previous, &thread).await?;
    }
    let run = Run {
        feedback_version: 2,
        id: Uuid::new_v4(),
        thread_id: thread,
        plan,
        feedback: Vec::new(),
        submitted: false,
        revision: 0,
        notification: NotificationStatus::NotConfigured,
    };
    persist(root, &run).await?;
    Ok(run)
}

struct Site {
    root: PathBuf,
    run: Mutex<Run>,
    token: String,
    origin: String,
    active: Arc<AtomicBool>,
    notifier: Option<SubmissionNotifier>,
}

#[allow(clippy::await_holding_invalid_type)] // Serialize submission and retries with the persisted delivery status.
async fn notify_submission(site: &Site) -> Result<Run> {
    let mut run = site.run.lock().await;
    if !run.submitted {
        bail!("run has not been submitted");
    }
    if run.notification == NotificationStatus::Queued {
        return Ok(run.clone());
    }
    if !site.active.load(Ordering::Acquire) {
        bail!("site is closed");
    }
    let notifier = site
        .notifier
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no original session connected"))?;
    notifier(SubmissionNotice {
        run_id: run.id,
        thread_id: run.thread_id.clone(),
    })
    .await?;
    let mut delivered = run.clone();
    delivered.notification = NotificationStatus::Queued;
    // If this write fails, keep pending on disk so reopening can retry.
    persist(&site.root, &delivered).await?;
    *run = delivered;
    Ok(run.clone())
}

async fn retry_notification(State(site): State<Arc<Site>>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &site) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !site.run.lock().await.submitted {
        return (StatusCode::CONFLICT, "验收尚未提交。").into_response();
    }
    match notify_submission(&site).await {
        Ok(run) => Json(run).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "验收结果已保存，但原会话通知未成功，请稍后重试。",
        )
            .into_response(),
    }
}

fn authorized(headers: &HeaderMap, site: &Site) -> bool {
    site.active.load(Ordering::Acquire)
        && headers.get("authorization").and_then(|h| h.to_str().ok())
            == Some(&format!("Bearer {}", site.token))
        && headers
            .get("origin")
            .is_none_or(|h| h.to_str().ok() == Some(site.origin.as_str()))
        && headers.get("host").and_then(|h| h.to_str().ok()) == site.origin.strip_prefix("http://")
}

async fn read_run(State(site): State<Arc<Site>>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &site) {
        return StatusCode::FORBIDDEN.into_response();
    }
    Json(site.run.lock().await.clone()).into_response()
}

#[allow(clippy::await_holding_invalid_type)] // Serialize persistence with revision checks; this is an async mutex, not a blocking lock.
async fn save_run(
    State(site): State<Arc<Site>>,
    headers: HeaderMap,
    Json(update): Json<Update>,
) -> Response {
    if !authorized(&headers, &site) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut run = site.run.lock().await;
    if !authorized(&headers, &site) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut next = run.clone();
    if let Err(error) = next.update(update) {
        return (StatusCode::CONFLICT, error.to_string()).into_response();
    }
    if next.submitted && site.notifier.is_some() {
        next.notification = NotificationStatus::Pending;
    }
    if persist(&site.root, &next).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    *run = next;
    let saved = run.clone();
    drop(run);
    if saved.submitted && site.notifier.is_some() {
        // A delivery failure must not turn a durable submission into "save failed".
        return Json(notify_submission(&site).await.unwrap_or(saved)).into_response();
    }
    Json(saved).into_response()
}

/// The owner must retain this handle. Dropping it closes the listener.
pub struct Server {
    pub url: String,
    task: tokio::task::JoinHandle<()>,
    active: Arc<AtomicBool>,
}

impl Drop for Server {
    fn drop(&mut self) {
        // Already accepted keep-alive connections must lose their capability too.
        self.active.store(false, Ordering::Release);
        self.task.abort();
    }
}

pub async fn serve(root: PathBuf, run: Run) -> Result<Server> {
    serve_with_notifier(root, run, None).await
}

pub async fn serve_with_notifier(
    root: PathBuf,
    run: Run,
    notifier: Option<SubmissionNotifier>,
) -> Result<Server> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let token = Uuid::new_v4().to_string();
    let url = format!("{origin}/#{token}");
    let active = Arc::new(AtomicBool::new(true));
    let recover_pending =
        run.submitted && run.notification == NotificationStatus::Pending && notifier.is_some();
    let site = Arc::new(Site {
        root,
        run: Mutex::new(run),
        token,
        origin,
        active: active.clone(),
        notifier,
    });
    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("index.html")) }))
        .route("/api/run", get(read_run).post(save_run))
        .route("/api/notify", axum::routing::post(retry_notification))
        .route("/app.js", get(|| async { ([("content-type", "text/javascript; charset=utf-8")], include_str!("app.js")) }))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(axum::middleware::map_response(|mut response: Response| async move {
            let headers = response.headers_mut();
            headers.insert("cache-control", axum::http::HeaderValue::from_static("no-store"));
            headers.insert("referrer-policy", axum::http::HeaderValue::from_static("no-referrer"));
            headers.insert("x-content-type-options", axum::http::HeaderValue::from_static("nosniff"));
            headers.insert("content-security-policy", axum::http::HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"));
            response
        }))
        .with_state(site.clone());
    if recover_pending {
        tokio::spawn(async move {
            let _ = notify_submission(&site).await;
        });
    }
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(Server { url, task, active })
}

#[cfg(test)]
mod step_feedback_tests {
    use super::*;
    use serde_json::json;

    fn run() -> Result<Run> {
        Ok(serde_json::from_value(json!({
            "id":Uuid::new_v4(), "thread_id":"t", "feedback_version":2,
            "submitted":false,"revision":0,"feedback":[],
            "plan":{"title":"test","cases":[{"id":"c","title":"case","reason":"visual","prerequisites":"",
                "steps":[{"action":"one","expected":"one"},{"action":"two","expected":"two"}]}]}
        }))?)
    }

    fn update(states: &[(&str, &str)], submit: bool) -> Result<Update> {
        Ok(serde_json::from_value(
            json!({"revision":0,"submit":submit,"feedback":[{
                "case_id":"c","outcome":"passed","actual":"optional note","evidence":"",
                "steps":states.iter().enumerate().map(|(i,(outcome,actual))| json!({"step_index":i+1,"outcome":outcome,"actual":actual,"evidence":"link"})).collect::<Vec<_>>()
            }]}),
        )?)
    }

    #[test]
    fn aggregation_is_server_owned_and_not_run_is_never_passed() -> Result<()> {
        for (states, expected, can_submit) in [
            (vec![("passed", ""), ("passed", "")], Outcome::Passed, true),
            (
                vec![("passed", ""), ("not_run", "")],
                Outcome::NotRun,
                false,
            ),
            (
                vec![("blocked", "no device"), ("not_run", "")],
                Outcome::Blocked,
                true,
            ),
            (
                vec![("failed", "bad UI"), ("blocked", "no device")],
                Outcome::Failed,
                true,
            ),
        ] {
            let mut draft = run()?;
            draft.update(update(&states, false)?)?;
            assert_eq!(draft.feedback[0].outcome, expected);
            assert_eq!(run()?.update(update(&states, true)?).is_ok(), can_submit);
        }
        Ok(())
    }

    #[test]
    fn validates_step_coverage_explanations_and_size() -> Result<()> {
        for states in [
            vec![("passed", "")],
            vec![("failed", ""), ("passed", "")],
            vec![("blocked", ""), ("passed", "")],
        ] {
            assert!(run()?.update(update(&states, false)?).is_err());
        }
        for index in [0, 1, 3] {
            let mut input = update(&[("passed", ""), ("passed", "")], false)?;
            input.feedback[0]
                .steps
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("steps"))?[1]
                .step_index = index;
            assert!(run()?.update(input).is_err());
        }
        let oversized = "x".repeat(8193);
        assert!(
            run()?
                .update(update(&[("failed", &oversized), ("passed", "")], false)?)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn legacy_feedback_is_preserved_but_cannot_be_submitted_as_steps() -> Result<()> {
        let mut value = serde_json::to_value(run()?)?;
        value
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("object"))?
            .remove("feedback_version");
        value["feedback"] = json!([{"case_id":"c","outcome":"failed","actual":"old observation","evidence":"old link"}]);
        let mut legacy: Run = serde_json::from_value(value)?;
        assert_eq!(legacy.feedback_version, 1);
        assert!(legacy.feedback[0].steps.is_none());
        let saved = json!({"revision":0,"feedback":legacy.feedback,"submit":false});
        legacy.update(serde_json::from_value(saved.clone())?)?;
        assert_eq!(legacy.feedback[0].actual, "old observation");
        let mut submission = saved;
        submission["revision"] = json!(1);
        submission["submit"] = json!(true);
        assert!(legacy.update(serde_json::from_value(submission.clone())?).is_err());
        let mut new_run = run()?;
        submission["revision"] = json!(0);
        submission["submit"] = json!(false);
        assert!(new_run.update(serde_json::from_value(submission)?).is_err());
        Ok(())
    }
}
