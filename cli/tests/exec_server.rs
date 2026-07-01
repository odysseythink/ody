use std::path::Path;

use anyhow::Result;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use tempfile::TempDir;

fn ody_command(ody_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(ody_utils_cargo_bin::cargo_bin("ody")?);
    cmd.env("ODY_HOME", ody_home);
    Ok(cmd)
}

#[test]
fn strict_config_rejects_unknown_config_fields_for_exec_server() -> Result<()> {
    let ody_home = TempDir::new()?;
    std::fs::write(
        ody_home.path().join("config.toml"),
        r#"
foo = "bar"
"#,
    )?;

    let mut cmd = ody_command(ody_home.path())?;
    cmd.args([
        "exec-server",
        "--strict-config",
        "--listen",
        "http://127.0.0.1:0",
    ])
    .assert()
    .failure()
    .stderr(contains("unknown configuration field"));

    Ok(())
}

#[test]
fn local_exec_server_ignores_invalid_config_without_strict_config() -> Result<()> {
    let ody_home = TempDir::new()?;
    std::fs::write(ody_home.path().join("config.toml"), "not valid toml = [")?;

    let mut cmd = ody_command(ody_home.path())?;
    cmd.args(["exec-server", "--listen", "stdio"])
        .assert()
        .success()
        .stderr(contains("not valid toml").not());

    Ok(())
}
