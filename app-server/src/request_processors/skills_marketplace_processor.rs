use super::*;
use ody_app_server_protocol::ServerNotification;
use ody_app_server_protocol::SkillsChangedNotification;
use ody_app_server_protocol::SkillsMarketplaceEntry;
use ody_app_server_protocol::SkillsMarketplaceInstallParams;
use ody_app_server_protocol::SkillsMarketplaceInstallResponse;
use ody_app_server_protocol::SkillsDeleteParams;
use ody_app_server_protocol::SkillsDeleteResponse;
use ody_app_server_protocol::SkillsMarketplaceSearchParams;
use ody_app_server_protocol::SkillsMarketplaceSearchResponse;
use ody_app_server_protocol::SkillsUpgradeParams;
use ody_app_server_protocol::SkillsUpgradeResponse;
use ody_core::config::Config;
use std::process::Command;
use std::time::Duration;

const SKILLS_SEARCH_API_URL: &str = "https://skills.sh/api/search";
const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_SKILL_NAME_LEN: usize = 64;

/// Backs the `skills/marketplace/search`, `skills/marketplace/install`,
/// `skills/delete`, and `skills/upgrade` requests. Search proxies the public
/// skills.sh catalog; install copies a skill from a GitHub repository into
/// `$ODY_HOME/skills/<name>` so the regular skill discovery picks it up on
/// the next (cache-free) scan. Delete removes an installed skill directory;
/// upgrade re-pulls it from its recorded (or supplied) GitHub source.
pub(crate) struct SkillsMarketplaceRequestProcessor {
    config: Arc<Config>,
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
}

impl SkillsMarketplaceRequestProcessor {
    pub(crate) fn new(
        config: Arc<Config>,
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
    ) -> Self {
        Self {
            config,
            thread_manager,
            outgoing,
        }
    }

    pub(crate) async fn search(
        &self,
        params: SkillsMarketplaceSearchParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let limit = params.limit.unwrap_or(20).min(100);
        let client = reqwest::Client::builder()
            .timeout(SEARCH_TIMEOUT)
            .build()
            .map_err(|err| internal_error(format!("failed to build http client: {err}")))?;
        let limit_string = limit.to_string();
        let response = client
            .get(SKILLS_SEARCH_API_URL)
            .query(&[("q", params.query.as_str()), ("limit", limit_string.as_str())])
            .send()
            .await
            .map_err(|err| internal_error(format!("skills marketplace search failed: {err}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(internal_error(format!(
                "skills marketplace search returned status {status}"
            )));
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|err| internal_error(format!("failed to parse marketplace response: {err}")))?;
        let skills = body
            .get("skills")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(skills_marketplace_entry_from_json)
            .collect();
        Ok(Some(SkillsMarketplaceSearchResponse { skills }.into()))
    }

    pub(crate) async fn install(
        &self,
        params: SkillsMarketplaceInstallParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let skills_root = self.config.ody_home.join("skills");
        let outcome = tokio::task::spawn_blocking({
            let skills_root = skills_root.clone();
            let source = params.source.clone();
            let name = params.name.clone();
            let path = params.path.clone();
            move || install_skill_from_github(&skills_root, &source, name.as_deref(), path.as_deref())
        })
        .await
        .map_err(|err| internal_error(format!("skill install task failed: {err}")))?;

        let (name, path) = outcome.map_err(invalid_request)?;

        self.thread_manager.skills_service().clear_cache();
        self.outgoing
            .send_server_notification(ServerNotification::SkillsChanged(
                SkillsChangedNotification {},
            ))
            .await;

        Ok(Some(SkillsMarketplaceInstallResponse { name, path }.into()))
    }

    pub(crate) async fn delete(
        &self,
        params: SkillsDeleteParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let skills_root = self.config.ody_home.join("skills");
        tokio::task::spawn_blocking(move || delete_skill_dir(&skills_root, &params.name))
            .await
            .map_err(|err| internal_error(format!("skill delete task failed: {err}")))?
            .map_err(invalid_request)?;
        self.thread_manager.skills_service().clear_cache();
        self.outgoing
            .send_server_notification(ServerNotification::SkillsChanged(
                SkillsChangedNotification {},
            ))
            .await;
        Ok(Some(SkillsDeleteResponse {}.into()))
    }

    pub(crate) async fn upgrade(
        &self,
        params: SkillsUpgradeParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let skills_root = self.config.ody_home.join("skills");
        let outcome = tokio::task::spawn_blocking({
            let skills_root = skills_root.clone();
            let name = params.name.clone();
            let source = params.source.clone();
            let path = params.path.clone();
            move || upgrade_skill_from_github(&skills_root, &name, source, path)
        })
        .await
        .map_err(|err| internal_error(format!("skill upgrade task failed: {err}")))?;
        let (name, path) = outcome.map_err(invalid_request)?;
        self.thread_manager.skills_service().clear_cache();
        self.outgoing
            .send_server_notification(ServerNotification::SkillsChanged(
                SkillsChangedNotification {},
            ))
            .await;
        Ok(Some(SkillsUpgradeResponse { name, path }.into()))
    }
}

fn skills_marketplace_entry_from_json(value: serde_json::Value) -> Option<SkillsMarketplaceEntry> {
    let object = value.as_object()?;
    let string_field = |key: &str| object.get(key).and_then(serde_json::Value::as_str);
    let id = string_field("id")?.to_string();
    let skill_id = string_field("skill_id")
        .or_else(|| string_field("skillId"))
        .unwrap_or(id.as_str())
        .to_string();
    let name = string_field("name")?.to_string();
    let source = string_field("source")?.to_string();
    let installs = object
        .get("installs")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
        })
        .unwrap_or(0);
    let description = string_field("description").map(str::to_string);
    Some(SkillsMarketplaceEntry {
        id,
        skill_id,
        name,
        installs,
        source,
        description,
    })
}

fn install_skill_from_github(
    skills_root: &std::path::Path,
    source: &str,
    name: Option<&str>,
    path: Option<&str>,
) -> Result<(String, AbsolutePathBuf), String> {
    let repo_url = normalize_github_url(source)?;
    let name = name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| repo_url.split('/').next_back().map(ToString::to_string))
        .ok_or_else(|| "unable to resolve skill name".to_string())?;
    validate_skill_name(&name)?;

    std::fs::create_dir_all(skills_root)
        .map_err(|err| format!("failed to create skills directory: {err}"))?;
    let target = skills_root.join(&name);
    if target.exists() {
        return Err(format!("skill '{name}' is already installed"));
    }

    let temp = skills_root.join(format!(".install-tmp-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let clone = Command::new("git")
        .args(["clone", "--depth", "1", &repo_url])
        .arg(&temp)
        .output()
        .map_err(|err| format!("failed to run git: {err}"));
    let output = match clone {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(format!(
                "git clone failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Err(message) => {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(message);
        }
    };
    drop(output);

    let install_result = locate_skill_dir(&temp, &name, path)
        .and_then(|skill_dir| copy_skill_dir(&skill_dir, &target));
    let _ = std::fs::remove_dir_all(&temp);
    install_result?;
    // Record the origin so `skills/upgrade` can re-pull without the caller
    // remembering the URL; matches odyBox's legacy source.json shape.
    write_skill_source(&target, &repo_slug_from_url(&repo_url)?, path)?;

    let path = AbsolutePathBuf::from_absolute_path(&target)
        .map_err(|err| format!("failed to resolve installed skill path: {err}"))?;
    Ok((name, path))
}

/// Re-pulls an installed skill from GitHub and replaces its directory. The
/// replacement is staged to a temp directory first so a failed clone or copy
/// leaves the existing skill untouched.
fn upgrade_skill_from_github(
    skills_root: &std::path::Path,
    name: &str,
    source: Option<String>,
    path: Option<String>,
) -> Result<(String, AbsolutePathBuf), String> {
    validate_skill_name(name)?;
    std::fs::create_dir_all(skills_root)
        .map_err(|err| format!("failed to create skills directory: {err}"))?;
    let target = skills_root.join(name);
    if !target.is_dir() {
        return Err(format!("skill '{name}' is not installed"));
    }
    let source = resolve_upgrade_source(source, &target)?;
    let repo_url = normalize_github_url(&source)?;
    let repo_slug = repo_slug_from_url(&repo_url)?;

    let temp = skills_root.join(format!(".upgrade-tmp-{}-{name}", std::process::id()));
    let staged = skills_root.join(format!(".upgrade-staged-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let _ = std::fs::remove_dir_all(&staged);
    let clone = Command::new("git")
        .args(["clone", "--depth", "1", &repo_url])
        .arg(&temp)
        .output()
        .map_err(|err| format!("failed to run git: {err}"));
    let output = match clone {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(format!(
                "git clone failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Err(message) => {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(message);
        }
    };
    drop(output);

    let staged_result = locate_skill_dir(&temp, name, path.as_deref())
        .and_then(|skill_dir| copy_skill_dir(&skill_dir, &staged));
    let _ = std::fs::remove_dir_all(&temp);
    staged_result?;

    // Same-filesystem rename cannot fail once staging succeeded; the old
    // directory is only removed after the replacement is complete on disk.
    std::fs::remove_dir_all(&target).map_err(|err| format!("failed to remove old skill: {err}"))?;
    std::fs::rename(&staged, &target).map_err(|err| format!("failed to swap in new skill: {err}"))?;
    write_skill_source(&target, &repo_slug, path.as_deref())?;

    let path = AbsolutePathBuf::from_absolute_path(&target)
        .map_err(|err| format!("failed to resolve installed skill path: {err}"))?;
    Ok((name.to_string(), path))
}

/// Removes an installed skill directory. Names are validated (`..`, `/`, and
/// `\` rejected) and the target must be a real directory inside the skills
/// root, so the write surface is exactly `$ODY_HOME/skills/<name>`.
fn delete_skill_dir(skills_root: &std::path::Path, name: &str) -> Result<(), String> {
    validate_skill_name(name)?;
    let root = skills_root
        .canonicalize()
        .map_err(|err| format!("failed to resolve skills directory: {err}"))?;
    let target = root.join(name);
    if !target.is_dir() {
        return Err(format!("skill '{name}' is not installed"));
    }
    let canonical = target
        .canonicalize()
        .map_err(|err| format!("failed to resolve skill directory: {err}"))?;
    if !canonical.starts_with(&root) {
        return Err(format!("invalid skill name: {name}"));
    }
    std::fs::remove_dir_all(&canonical).map_err(|err| format!("failed to delete skill: {err}"))
}

#[derive(serde::Deserialize)]
struct SkillSourceJson {
    repo: Option<String>,
}

fn write_skill_source(
    skill_dir: &std::path::Path,
    repo: &str,
    skill_path: Option<&str>,
) -> Result<(), String> {
    let mut value = serde_json::json!({
        "type": "github",
        "repo": repo,
        "installedAt": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(skill_path) = skill_path.map(str::trim).filter(|value| !value.is_empty()) {
        value["skillPath"] = serde_json::json!(skill_path);
    }
    let body = serde_json::to_string_pretty(&value).map_err(|err| format!("invalid source: {err}"))?;
    std::fs::write(skill_dir.join("source.json"), body)
        .map_err(|err| format!("failed to record skill source: {err}"))
}

/// Reads the GitHub source recorded by `write_skill_source` (or odyBox's
/// legacy installer, whose format we mirror) back into a cloneable URL.
fn read_upgrade_source(skill_dir: &std::path::Path) -> Result<String, String> {
    let body = std::fs::read_to_string(skill_dir.join("source.json"))
        .map_err(|err| format!("no source recorded for this skill ({err})"))?;
    let source: SkillSourceJson =
        serde_json::from_str(&body).map_err(|err| format!("invalid source.json: {err}"))?;
    let repo = source
        .repo
        .filter(|repo| !repo.trim().is_empty())
        .ok_or_else(|| "no source recorded for this skill".to_string())?;
    if repo.starts_with("http://") || repo.starts_with("https://") {
        Ok(repo)
    } else {
        Ok(format!("https://github.com/{repo}"))
    }
}

/// Params win over the recorded source so a moved repository can be adopted.
fn resolve_upgrade_source(
    source: Option<String>,
    skill_dir: &std::path::Path,
) -> Result<String, String> {
    if let Some(source) = source
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Ok(source);
    }
    read_upgrade_source(skill_dir)
}

/// `https://github.com/owner/repo(.git)` -> `owner/repo`.
fn repo_slug_from_url(repo_url: &str) -> Result<String, String> {
    let path = repo_url
        .strip_prefix("https://github.com/")
        .ok_or_else(|| format!("unsupported skill source: {repo_url}"))?;
    Ok(path.strip_suffix(".git").unwrap_or(path).to_string())
}

#[cfg(test)]
#[path = "skills_marketplace_processor_tests.rs"]
mod skills_marketplace_processor_tests;

/// Accepts `https://github.com/owner/repo` (with optional trailing slash and
/// `.git`) and returns an https clone URL. Other hosts are rejected to keep
/// the install surface narrow.
fn normalize_github_url(source: &str) -> Result<String, String> {
    let trimmed = source.trim();
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let path = without_scheme
        .strip_prefix("github.com/")
        .ok_or_else(|| format!("unsupported skill source (github.com required): {trimmed}"))?;
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if segments.len() < 2 {
        return Err(format!("invalid github source: {trimmed}"));
    }
    let owner = segments[0];
    let repo = segments[1].strip_suffix(".git").unwrap_or(segments[1]);
    Ok(format!("https://github.com/{owner}/{repo}.git"))
}

fn validate_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_SKILL_NAME_LEN {
        return Err(format!("invalid skill name: {name}"));
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err(format!("invalid skill name: {name}"));
    }
    Ok(())
}

fn locate_skill_dir(
    repo: &std::path::Path,
    name: &str,
    path: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(path) = path.map(str::trim).filter(|value| !value.is_empty()) {
        candidates.push(path.to_string());
    }
    candidates.extend([
        format!("skills/{name}"),
        format!(".claude/skills/{name}"),
        name.to_string(),
        String::new(),
    ]);
    for candidate in candidates {
        let dir = repo.join(&candidate);
        if dir.join("SKILL.md").is_file() {
            return Ok(dir);
        }
    }
    Err(format!("no SKILL.md found for skill '{name}' in {}", repo.display()))
}

fn copy_skill_dir(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|err| format!("failed to create skill dir: {err}"))?;
    for entry in std::fs::read_dir(from).map_err(|err| format!("failed to read skill dir: {err}"))? {
        let entry = entry.map_err(|err| format!("failed to read skill dir entry: {err}"))?;
        let file_name = entry.file_name();
        // Never copy nested VCS metadata into the installed skill.
        if file_name == ".git" {
            continue;
        }
        let source = entry.path();
        let destination = to.join(&file_name);
        if source.is_dir() {
            copy_dir_recursive(&source, &destination)?;
        } else {
            std::fs::copy(&source, &destination)
                .map_err(|err| format!("failed to copy {}: {err}", source.display()))?;
        }
    }
    Ok(())
}

fn copy_dir_recursive(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|err| format!("failed to create dir: {err}"))?;
    for entry in std::fs::read_dir(from).map_err(|err| format!("failed to read dir: {err}"))? {
        let entry = entry.map_err(|err| format!("failed to read dir entry: {err}"))?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        if source.is_dir() {
            copy_dir_recursive(&source, &destination)?;
        } else {
            std::fs::copy(&source, &destination)
                .map_err(|err| format!("failed to copy {}: {err}", source.display()))?;
        }
    }
    Ok(())
}
