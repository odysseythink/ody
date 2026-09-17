use ody_human_acceptance::{
    Case, NotificationStatus, Plan, Run, Step, SubmissionNotifier, create, load,
    serve_with_notifier,
};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn plan() -> Plan {
    Plan {
        title: "acceptance".into(),
        previous_run: None,
        cases: vec![Case {
            id: "ui".into(),
            title: "manual".into(),
            reason: "visual".into(),
            prerequisites: String::new(),
            steps: vec![Step {
                action: "observe".into(),
                expected: "clear".into(),
            }],
        }],
    }
}

#[tokio::test]
async fn notifications_are_after_commit_idempotent_and_retryable() -> anyhow::Result<()> {
    let root = tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir())?)?;
    let run = create(root.path(), "thread".into(), plan()).await?;
    let count = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(AtomicBool::new(true));
    let notifier: SubmissionNotifier = {
        let count = count.clone();
        let fail = fail.clone();
        let root = root.path().to_path_buf();
        Arc::new(move |notice| {
            let count = count.clone();
            let fail = fail.clone();
            let root = root.clone();
            Box::pin(async move {
                // The result must already be durable before the host sees it.
                assert_eq!(notice.thread_id, "thread");
                assert!(
                    load(&root, notice.run_id, &notice.thread_id)
                        .await?
                        .submitted
                );
                count.fetch_add(1, Ordering::SeqCst);
                if fail.load(Ordering::SeqCst) {
                    anyhow::bail!("disconnected");
                }
                Ok(())
            })
        })
    };
    let server =
        serve_with_notifier(root.path().into(), run.clone(), Some(notifier.clone())).await?;
    let url = url::Parts::from_url(&server.url);
    let client = reqwest::Client::builder().no_proxy().build()?;
    let draft = json!({"revision":0,"feedback":[],"submit":false});
    assert_eq!(
        client
            .post(&url.api)
            .bearer_auth(&url.token)
            .json(&draft)
            .send()
            .await?
            .status(),
        200
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(client.post(&url.retry).send().await?.status(), 403);
    assert_eq!(
        client
            .post(&url.retry)
            .bearer_auth(&url.token)
            .send()
            .await?
            .status(),
        409
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let submitted = json!({"revision":1,"feedback":[{"case_id":"ui","outcome":"failed","actual":"","evidence":"","steps":[{"step_index":1,"outcome":"failed","actual":"bad layout","evidence":""}]}],"submit":true});
    let saved: Run = client
        .post(&url.api)
        .bearer_auth(&url.token)
        .json(&submitted)
        .send()
        .await?
        .json()
        .await?;
    assert!(saved.submitted);
    assert_eq!(saved.notification, NotificationStatus::Pending);
    assert_eq!(
        load(root.path(), run.id, "thread").await?.notification,
        NotificationStatus::Pending
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    fail.store(false, Ordering::SeqCst);
    let first = client.post(&url.retry).bearer_auth(&url.token).send();
    let second = client.post(&url.retry).bearer_auth(&url.token).send();
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first?.status(), 200);
    assert_eq!(second?.status(), 200);
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(
        load(root.path(), run.id, "thread").await?.notification,
        NotificationStatus::Queued
    );
    drop(server);
    let reopened = serve_with_notifier(
        root.path().into(),
        load(root.path(), run.id, "thread").await?,
        Some(notifier),
    )
    .await?;
    let new = url::Parts::from_url(&reopened.url);
    client
        .post(&new.retry)
        .bearer_auth(&new.token)
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(count.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn reopening_a_pending_run_recovers_notification() -> anyhow::Result<()> {
    let root = tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir())?)?;
    let run = create(root.path(), "thread".into(), plan()).await?;
    let failed: SubmissionNotifier = Arc::new(|_| Box::pin(async { anyhow::bail!("closed") }));
    let server = serve_with_notifier(root.path().into(), run.clone(), Some(failed)).await?;
    let url = url::Parts::from_url(&server.url);
    let client = reqwest::Client::builder().no_proxy().build()?;
    let submitted = json!({"revision":0,"feedback":[{"case_id":"ui","outcome":"passed","actual":"","evidence":"","steps":[{"step_index":1,"outcome":"passed","actual":"","evidence":""}]}],"submit":true});
    client
        .post(&url.api)
        .bearer_auth(&url.token)
        .json(&submitted)
        .send()
        .await?
        .error_for_status()?;
    drop(server);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let notifier: SubmissionNotifier = Arc::new(move |notice| {
        let tx = tx.clone();
        Box::pin(async move {
            tx.send(notice)?;
            Ok(())
        })
    });
    let _reopened = serve_with_notifier(
        root.path().into(),
        load(root.path(), run.id, "thread").await?,
        Some(notifier),
    )
    .await?;
    let notice = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await?
        .ok_or_else(|| anyhow::anyhow!("no notice"))?;
    assert_eq!(notice.run_id, run.id);
    Ok(())
}

mod url {
    pub struct Parts {
        pub api: String,
        pub retry: String,
        pub token: String,
    }
    impl Parts {
        pub fn from_url(url: &str) -> Self {
            let (origin, token) = url.split_once("/#").unwrap_or_default();
            Self {
                api: format!("{origin}/api/run"),
                retry: format!("{origin}/api/notify"),
                token: token.into(),
            }
        }
    }
}
