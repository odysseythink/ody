#![allow(clippy::unwrap_used)]

use ody_human_acceptance::{Case, Plan, Run, Step, create, load, serve};
use serde_json::json;

fn temporary() -> anyhow::Result<tempfile::TempDir> {
    Ok(tempfile::tempdir_in(std::fs::canonicalize(
        std::env::temp_dir(),
    )?)?)
}

fn plan() -> Plan {
    Plan {
        title: "手动验收 <script>".into(),
        previous_run: None,
        cases: vec![Case {
            id: "save".into(),
            title: "输入不丢失".into(),
            reason: "人工观察交互".into(),
            prerequisites: "编辑页面".into(),
            steps: vec![Step {
                action: "断网保存".into(),
                expected: "输入保留".into(),
            }],
        }],
    }
}

#[tokio::test]
async fn durable_runs_are_thread_scoped_and_retests_keep_history() -> anyhow::Result<()> {
    let root = temporary()?;
    let first = create(root.path(), "thread".into(), plan()).await?;
    assert_eq!(load(root.path(), first.id, "thread").await?.revision, 0);
    assert!(load(root.path(), first.id, "other").await.is_err());
    let mut retest = plan();
    retest.previous_run = Some(first.id);
    let second = create(root.path(), "thread".into(), retest).await?;
    assert_ne!(first.id, second.id);
    assert_eq!(
        load(root.path(), second.id, "thread")
            .await?
            .plan
            .previous_run,
        Some(first.id)
    );
    assert!(
        create(root.path(), "other".into(), {
            let mut p = plan();
            p.previous_run = Some(first.id);
            p
        })
        .await
        .is_err()
    );
    Ok(())
}

#[test]
fn validates_cases_and_steps() {
    let mut p = plan();
    assert!(p.validate().is_ok());
    p.cases.push(p.cases[0].clone());
    assert!(p.validate().is_err());
    p.cases.pop();
    p.cases[0].steps[0].expected.clear();
    assert!(p.validate().is_err());
}

#[tokio::test]
async fn pending_discovery_filters_submitted_and_rejects_wrong_threads() -> anyhow::Result<()> {
    let root = temporary()?;
    assert!(
        ody_human_acceptance::pending_runs(&root.path().join("missing"), "thread")
            .await?
            .is_empty()
    );
    let first = create(root.path(), "thread".into(), plan()).await?;
    assert_eq!(
        ody_human_acceptance::pending_runs(root.path(), "thread").await?,
        vec![first.id]
    );
    assert!(
        ody_human_acceptance::pending_runs(root.path(), "other")
            .await
            .is_err()
    );
    let mut submitted = first.clone();
    submitted.submitted = true;
    tokio::fs::write(
        root.path().join(format!("{}.json", first.id)),
        serde_json::to_vec(&submitted)?,
    )
    .await?;
    assert!(
        ody_human_acceptance::pending_runs(root.path(), "thread")
            .await?
            .is_empty()
    );
    let second = create(root.path(), "thread".into(), plan()).await?;
    assert_eq!(
        ody_human_acceptance::pending_runs(root.path(), "thread").await?,
        vec![second.id]
    );
    Ok(())
}

#[tokio::test]
async fn pending_discovery_is_newest_first_and_bounded() -> anyhow::Result<()> {
    let root = temporary()?;
    let mut ids = Vec::new();
    for index in 0..22 {
        let run = create(root.path(), "thread".into(), plan()).await?;
        std::fs::File::open(root.path().join(format!("{}.json", run.id)))?
            .set_times(std::fs::FileTimes::new().set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(100 + index),
            ))?;
        ids.push(run.id);
    }
    ids.reverse();
    ids.truncate(20);
    assert_eq!(
        ody_human_acceptance::pending_runs(root.path(), "thread").await?,
        ids
    );
    Ok(())
}

#[tokio::test]
async fn api_auth_drafts_submission_and_immutability() -> anyhow::Result<()> {
    let root = temporary()?;
    let run = create(root.path(), "thread".into(), plan()).await?;
    let server = serve(root.path().into(), run.clone()).await?;
    let url = server.url.clone();
    let (origin, token) = url.split_once("/#").unwrap();
    let api = format!("{origin}/api/run");
    let client = reqwest::Client::builder().no_proxy().build()?;
    assert_eq!(client.get(&api).send().await?.status(), 403);
    assert_eq!(
        client.get(&api).bearer_auth("wrong").send().await?.status(),
        403
    );
    assert_eq!(
        client
            .get(&api)
            .bearer_auth(token)
            .header("Origin", "https://evil.example")
            .send()
            .await?
            .status(),
        403
    );
    assert_eq!(
        client
            .get(&api)
            .bearer_auth(token)
            .header("Host", "evil.example")
            .send()
            .await?
            .status(),
        403
    );
    let page = client.get(origin).send().await?;
    assert!(page.headers().get("content-security-policy").is_some());
    assert!(!page.text().await?.contains("手动验收 <script>"));
    let empty = json!({"revision":0,"feedback":[],"submit":true});
    assert_eq!(
        client
            .post(&api)
            .bearer_auth(token)
            .json(&empty)
            .send()
            .await?
            .status(),
        409
    );
    let draft = json!({"revision":0,"feedback":[{"case_id":"save","outcome":"failed","actual":"","evidence":""}],"submit":false});
    assert_eq!(
        client
            .post(&api)
            .bearer_auth(token)
            .json(&draft)
            .send()
            .await?
            .status(),
        409
    );
    for case_id in ["unknown", "save"] {
        let draft = json!({"revision":0,"feedback":[{"case_id":case_id,"outcome":"blocked","actual":"没有设备","evidence":""}],"submit":false});
        let response = client
            .post(&api)
            .bearer_auth(token)
            .json(&draft)
            .send()
            .await?;
        assert_eq!(response.status(), if case_id == "save" { 200 } else { 409 });
    }
    let saved = load(root.path(), run.id, "thread").await?;
    assert_eq!(saved.revision, 1);
    assert!(!saved.submitted);
    let stale = json!({"revision":0,"feedback":saved.feedback,"submit":true});
    assert_eq!(
        client
            .post(&api)
            .bearer_auth(token)
            .json(&stale)
            .send()
            .await?
            .status(),
        409
    );
    let submitted = json!({"revision":1,"feedback":saved.feedback,"submit":true});
    assert_eq!(
        client
            .post(&api)
            .bearer_auth(token)
            .json(&submitted)
            .send()
            .await?
            .status(),
        200
    );
    assert!(load(root.path(), run.id, "thread").await?.submitted);
    assert_eq!(
        client
            .post(&api)
            .bearer_auth(token)
            .json(&submitted)
            .send()
            .await?
            .status(),
        409
    );
    drop(server);
    let reopened = serve(
        root.path().into(),
        load(root.path(), run.id, "thread").await?,
    )
    .await?;
    let (new_origin, new_token) = reopened.url.split_once("/#").unwrap();
    let read: Run = client
        .get(format!("{new_origin}/api/run"))
        .bearer_auth(new_token)
        .send()
        .await?
        .json()
        .await?;
    assert!(read.submitted);
    assert_ne!(token, new_token);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_symlinked_storage() -> anyhow::Result<()> {
    let root = temporary()?;
    let elsewhere = temporary()?;
    std::os::unix::fs::symlink(elsewhere.path(), root.path().join("linked"))?;
    assert!(
        ody_human_acceptance::pending_runs(&root.path().join("linked"), "thread")
            .await
            .is_err()
    );
    assert!(
        create(&root.path().join("linked"), "thread".into(), plan())
            .await
            .is_err()
    );
    Ok(())
}
