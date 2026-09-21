use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;

use tempfile::TempDir;

use crate::curated_sync::sync_curated_plugins_repo_from_url;
use crate::loader::curated_plugins_repo_path;

fn git_output(cwd: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .expect("spawn git")
}

fn git(cwd: &Path, args: &[&str]) {
    let output = git_output(cwd, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(cwd: &Path, args: &[&str]) -> String {
    let output = git_output(cwd, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A local bare repository standing in for the remote curated plugins repo, plus the
/// working repository used to advance it.
struct RemoteRepo {
    _tmp: TempDir,
    url: PathBuf,
    work: PathBuf,
}

impl RemoteRepo {
    fn new(with_marketplace_manifest: bool) -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let remote = tmp.path().join("remote.git");
        let work = tmp.path().join("work");
        git(tmp.path(), &["init", "--bare", remote.to_str().unwrap()]);
        git(tmp.path(), &["init", "-b", "main", work.to_str().unwrap()]);
        git(&work, &["config", "user.email", "test@example.com"]);
        git(&work, &["config", "user.name", "Test"]);
        // Fetching by commit sha requires the server to opt in; GitHub enables this for
        // branch tips and the sync relies on it, so mirror that here.
        git(&remote, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
        git(&remote, &["config", "uploadpack.allowTipSHA1InWant", "true"]);
        if with_marketplace_manifest {
            write_marketplace(&work);
        } else {
            std::fs::write(work.join("README.md"), "no marketplace manifest\n")
                .expect("write readme");
        }
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "init"]);
        git(&work, &["push", remote.to_str().unwrap(), "main"]);
        Self {
            _tmp: tmp,
            url: remote,
            work,
        }
    }

    fn advance(&self) {
        std::fs::write(self.work.join("UPDATE.txt"), "update\n").expect("write update");
        git(&self.work, &["add", "."]);
        git(&self.work, &["commit", "-m", "update"]);
        git(&self.work, &["push", self.url.to_str().unwrap(), "main"]);
    }
}

fn write_marketplace(work: &Path) {
    let manifest_dir = work.join(".agents/plugins");
    std::fs::create_dir_all(&manifest_dir).expect("mkdir manifest");
    std::fs::write(
        manifest_dir.join("marketplace.json"),
        r#"{
  "name": "openai-curated",
  "plugins": [
    {
      "name": "sample",
      "source": { "source": "local", "path": "./plugins/sample" }
    }
  ]
}"#,
    )
    .expect("write marketplace");
    let plugin_dir = work.join("plugins/sample/.ody-plugin");
    std::fs::create_dir_all(&plugin_dir).expect("mkdir plugin");
    std::fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"name":"sample"}"#,
    )
    .expect("write plugin manifest");
}

#[test]
fn first_sync_clones_remote_and_writes_sha() {
    let remote = RemoteRepo::new(true);
    let ody_home = TempDir::new().expect("ody home");

    let sha = sync_curated_plugins_repo_from_url(ody_home.path(), remote.url.to_str().unwrap())
        .expect("first sync");

    let repo = curated_plugins_repo_path(ody_home.path());
    assert!(
        repo.join(".agents/plugins/marketplace.json").is_file(),
        "marketplace manifest synced"
    );
    assert!(
        repo.join("plugins/sample/.ody-plugin/plugin.json")
            .is_file(),
        "plugin content synced"
    );
    let sha_file =
        std::fs::read_to_string(ody_home.path().join(".tmp/plugins.sha")).expect("sha file");
    assert_eq!(sha_file.trim(), sha);
}

#[test]
fn repeated_sync_is_noop_when_remote_unchanged() {
    let remote = RemoteRepo::new(true);
    let ody_home = TempDir::new().expect("ody home");
    let url = remote.url.to_str().unwrap();

    let first = sync_curated_plugins_repo_from_url(ody_home.path(), url).expect("first sync");
    let repo = curated_plugins_repo_path(ody_home.path());
    let head_before = git_stdout(&repo, &["rev-parse", "HEAD"]);

    let second = sync_curated_plugins_repo_from_url(ody_home.path(), url).expect("second sync");

    assert_eq!(first, second);
    assert_eq!(head_before, git_stdout(&repo, &["rev-parse", "HEAD"]));
}

#[test]
fn sync_updates_snapshot_when_remote_advances() {
    let remote = RemoteRepo::new(true);
    let ody_home = TempDir::new().expect("ody home");
    let url = remote.url.to_str().unwrap();

    let first = sync_curated_plugins_repo_from_url(ody_home.path(), url).expect("first sync");
    remote.advance();
    let second = sync_curated_plugins_repo_from_url(ody_home.path(), url).expect("second sync");

    assert_ne!(first, second);
    let repo = curated_plugins_repo_path(ody_home.path());
    assert!(repo.join("UPDATE.txt").is_file(), "new commit synced");
    assert_eq!(
        second,
        git_stdout(&repo, &["rev-parse", "HEAD"]),
        "snapshot HEAD matches remote sha"
    );
}

#[test]
fn sync_fails_without_marketplace_manifest() {
    let remote = RemoteRepo::new(false);
    let ody_home = TempDir::new().expect("ody home");

    let err = sync_curated_plugins_repo_from_url(ody_home.path(), remote.url.to_str().unwrap())
        .expect_err("sync must fail");

    assert!(
        err.contains("marketplace manifest"),
        "unexpected error: {err}"
    );
    assert!(
        !curated_plugins_repo_path(ody_home.path()).join(".git").is_dir(),
        "failed sync must not activate a snapshot"
    );
}
