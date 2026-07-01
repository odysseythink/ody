use anyhow::Result;
use predicates::str::contains;
use std::path::Path;
use tempfile::TempDir;

fn ody_command(ody_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(ody_utils_cargo_bin::cargo_bin("ody")?);
    cmd.env("ODY_HOME", ody_home);
    Ok(cmd)
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn update_does_not_start_interactive_prompt() -> Result<()> {
    let ody_home = TempDir::new()?;

    ody_command(ody_home.path())?
        .arg("update")
        .assert()
        .failure()
        .stderr(contains("`ody update` is not available in debug builds"));

    Ok(())
}
