use std::path::Path;

use anyhow::Result;
use tempfile::TempDir;

fn ody_command(ody_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(ody_utils_cargo_bin::cargo_bin("ody")?);
    cmd.env("ODY_HOME", ody_home);
    Ok(cmd)
}

#[test]
fn providers_lists_builtin_providers_and_capabilities() -> Result<()> {
    let ody_home = TempDir::new()?;
    let output = ody_command(ody_home.path())?
        .args(["providers"])
        .output()?;

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8(output.stdout)?;
    for id in ["kimi", "deepseek", "glm"] {
        assert!(
            stdout.contains(id),
            "expected provider {id} in stdout: {stdout}"
        );
    }
    // capabilities 会以 Debug 形式打印，至少应出现若干 capability 字段名
    assert!(stdout.contains("supports_websockets"));
    assert!(stdout.contains("web_search"));
    Ok(())
}

#[test]
fn providers_json_lists_providers() -> Result<()> {
    let ody_home = TempDir::new()?;
    let output = ody_command(ody_home.path())?
        .args(["providers", "--json"])
        .output()?;

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8(output.stdout)?;
    let value: serde_json::Value = serde_json::from_str(&stdout)?;
    let arr = value.as_array().expect("providers should be a JSON array");
    assert!(!arr.is_empty());
    let ids: Vec<&str> = arr.iter()
        .filter_map(|e| e["provider_id"].as_str())
        .collect();
    assert!(ids.contains(&"kimi"));
    assert!(ids.contains(&"kimi"));
    Ok(())
}

#[test]
fn providers_command_includes_user_configured_alias() -> Result<()> {
    let ody_home = TempDir::new()?;
    std::fs::write(
        ody_home.path().join("config.toml"),
        r#"
[providers.my_kimi]
type = "kimi"
api_key = "sk-test"
"#,
    )?;

    let output = ody_command(ody_home.path())?
        .args(["providers", "--json"])
        .output()?;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    let value: serde_json::Value = serde_json::from_str(&stdout)?;
    let arr = value.as_array().expect("providers should be a JSON array");
    let ids: Vec<&str> = arr
        .iter()
        .filter_map(|e| e["provider_id"].as_str())
        .collect();
    assert!(
        ids.contains(&"my_kimi"),
        "expected my_kimi in providers output: {stdout}"
    );

    Ok(())
}

