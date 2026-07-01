use anyhow::Result;
use predicates::str::contains;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::Path;
use tempfile::TempDir;

fn ody_command(ody_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(ody_utils_cargo_bin::cargo_bin("ody")?);
    cmd.env("ODY_HOME", ody_home);
    Ok(cmd)
}

#[tokio::test]
async fn marketplace_upgrade_runs_under_plugin() -> Result<()> {
    let ody_home = TempDir::new()?;

    ody_command(ody_home.path())?
        .args(["plugin", "marketplace", "upgrade"])
        .assert()
        .success()
        .stdout(contains("No configured Git marketplaces to upgrade."));

    Ok(())
}

#[tokio::test]
async fn marketplace_upgrade_json_prints_upgrade_outcome() -> Result<()> {
    let ody_home = TempDir::new()?;

    let assert = ody_command(ody_home.path())?
        .args(["plugin", "marketplace", "upgrade", "--json"])
        .assert()
        .success();
    let stdout = assert.get_output().stdout.as_slice();
    let actual: serde_json::Value = serde_json::from_slice(stdout)?;

    assert_eq!(
        actual,
        json!({
            "selectedMarketplaces": [],
            "upgradedRoots": [],
            "errors": [],
        })
    );

    Ok(())
}

#[tokio::test]
async fn marketplace_upgrade_no_longer_runs_at_top_level() -> Result<()> {
    let ody_home = TempDir::new()?;

    ody_command(ody_home.path())?
        .args(["marketplace", "upgrade"])
        .assert()
        .failure()
        .stderr(contains("unrecognized subcommand 'upgrade'"));

    Ok(())
}
